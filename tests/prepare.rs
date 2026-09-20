use crossload::prepare;
use image::{DynamicImage, ImageFormat};
use std::{
    fs,
    io::{Cursor, Read, Write},
};
use zip::{write::SimpleFileOptions, ZipArchive, ZipWriter};
fn fixture() -> (tempfile::TempDir, Vec<u8>) {
    let dir = tempfile::tempdir().unwrap();
    let mut jpeg = Cursor::new(Vec::new());
    DynamicImage::new_rgb8(1600, 2400)
        .write_to(&mut jpeg, ImageFormat::Jpeg)
        .unwrap();
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in [
        ("mimetype", b"application/epub+zip".to_vec()),
        ("META-INF/container.xml", b"<container><rootfiles><rootfile full-path='book.opf'/></rootfiles></container>".to_vec()),
        ("book.opf", b"<package xmlns:dc='http://purl.org/dc/elements/1.1/'><metadata><dc:title>A Book</dc:title><dc:creator>Writer</dc:creator></metadata><manifest><item id='c' href='chapter.xhtml' media-type='application/xhtml+xml'/><item id='i' href='cover.jpg' media-type='image/jpeg'/></manifest><spine><itemref idref='c'/></spine></package>".to_vec()),
        ("chapter.xhtml", b"<html><head/><body><p id='p'>Preserve every word.</p><img src='cover.jpg'/><a href='#p'>Back</a></body></html>".to_vec()),
        ("cover.jpg", jpeg.into_inner()),
    ] { zip.start_file(name, SimpleFileOptions::default()).unwrap(); zip.write_all(&bytes).unwrap(); }
    let bytes = zip.finish().unwrap().into_inner();
    fs::write(dir.path().join("input.epub"), &bytes).unwrap();
    (dir, bytes)
}
fn member(data: &[u8], name: &str) -> Vec<u8> {
    let mut zip = ZipArchive::new(Cursor::new(data)).unwrap();
    let mut out = Vec::new();
    zip.by_name(name).unwrap().read_to_end(&mut out).unwrap();
    out
}
#[test]
fn optimization_preserves_original_text_links_and_is_deterministic() {
    let (dir, original) = fixture();
    let input = dir.path().join("input.epub");
    let result = prepare::prepare(&input, Some(crossload::profile::X4), true).unwrap();
    assert_eq!(result.author, "Writer");
    assert_eq!(result.path.file_name().unwrap(), "A Book.epub");
    assert_eq!(result.images, 1);
    assert!(result.bytes < result.original_bytes);
    assert_eq!(fs::read(&input).unwrap(), original);
    let bytes = fs::read(&result.path).unwrap();
    for name in ["chapter.xhtml", "book.opf", "META-INF/container.xml"] {
        assert_eq!(member(&bytes, name), member(&original, name));
    }
    let image = image::load_from_memory(&member(&bytes, "cover.jpg")).unwrap();
    assert!(image.width() <= 480 && image.height() <= 800);
    let second = prepare::prepare(&input, Some(crossload::profile::X4), true).unwrap();
    assert_eq!(fs::read(second.path).unwrap(), bytes);
    let again = prepare::prepare(&result.path, Some(crossload::profile::X4), true).unwrap();
    assert_eq!(again.images, 0);
    assert_eq!(fs::read(again.path).unwrap(), bytes);
    let mut zip = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let first = zip.by_index(0).unwrap();
    assert_eq!(first.name(), "mimetype");
    assert_eq!(first.compression(), zip::CompressionMethod::Stored);
}
#[test]
fn opting_out_preserves_exact_bytes_and_filename() {
    let (dir, original) = fixture();
    let result = prepare::prepare(&dir.path().join("input.epub"), None, false).unwrap();
    assert_eq!(result.path.file_name().unwrap(), "input.epub");
    assert_eq!(fs::read(result.path).unwrap(), original);
}
#[test]
fn directory_names_are_bounded_and_cannot_escape_the_card() {
    assert_eq!(
        prepare::component("../../evil/path", "Unknown"),
        "_.._evil_path"
    );
    assert_eq!(prepare::component("...", "Unknown"), "Unknown");
    assert!(prepare::component(&"é".repeat(200), "Unknown").len() <= 120);
    let dir = tempfile::tempdir().unwrap();
    assert!(prepare::author_directory(&dir.path().join("missing"), "Author").is_err());
    let author = prepare::author_directory(dir.path(), "Author").unwrap();
    assert_eq!(
        prepare::author_directory(dir.path(), "AUTHOR").unwrap(),
        author
    );
    std::os::unix::fs::symlink(dir.path(), dir.path().join("Escape")).unwrap();
    assert!(prepare::author_directory(dir.path(), "Escape").is_err());
}
