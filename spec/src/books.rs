//! Books to test with: small, valid, and different from each other.
use std::io::{Cursor, Write};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

/// An EPUB called `title` by `author`, whose text is `text`.
pub fn epub_with(title: &str, author: &str, text: &str, compression: CompressionMethod) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(vec![]));
    for (name, value) in [
        ("mimetype", "application/epub+zip".to_owned()),
        ("META-INF/container.xml", "<container><rootfiles><rootfile full-path='book.opf'/></rootfiles></container>".into()),
        ("book.opf", format!("<package xmlns:dc='http://purl.org/dc/elements/1.1/'><metadata><dc:title>{title}</dc:title><dc:creator>{author}</dc:creator></metadata><manifest><item id='c' href='chapter.xhtml' media-type='application/xhtml+xml'/></manifest><spine><itemref idref='c'/></spine></package>")),
        ("chapter.xhtml", format!("<html><head/><body><p>{text}</p></body></html>")),
    ] {
        let options = SimpleFileOptions::default().compression_method(if name == "mimetype" {
            CompressionMethod::Stored
        } else {
            compression
        });
        zip.start_file(name, options).unwrap();
        zip.write_all(value.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// An EPUB called `title` by `author`.
pub fn epub(title: &str, author: &str) -> Vec<u8> {
    epub_with(title, author, title, CompressionMethod::Stored)
}

/// "Book" by "Author", with `text` in it: the same title, different books.
pub fn plain(text: &str, compression: CompressionMethod) -> Vec<u8> {
    epub_with("Book", "Author", text, compression)
}

pub fn pdf(text: &str) -> Vec<u8> {
    format!("%PDF-1.7\n1 0 obj<</Type/Catalog>>endobj\n{text}\ntrailer\n%%EOF\n").into_bytes()
}

pub fn cbz(pages: &[&str]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(vec![]));
    for name in pages {
        zip.start_file(
            *name,
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .unwrap();
        zip.write_all(name.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// The bytes of one member of a ZIP archive.
pub fn member(data: &[u8], name: &str) -> Vec<u8> {
    use std::io::Read;
    let mut archive = zip::ZipArchive::new(Cursor::new(data)).unwrap();
    let mut out = vec![];
    archive
        .by_name(name)
        .unwrap()
        .read_to_end(&mut out)
        .unwrap();
    out
}
