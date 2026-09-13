//! Device copies: bounded image decoding, stable EPUB resources and safe names.
use crate::epub;
use anyhow::{ensure, Context, Result};
use image::{imageops::FilterType, ImageFormat, ImageReader};
use std::{
    fs,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
};
use zip::{write::SimpleFileOptions, ZipArchive, ZipWriter};

pub struct Prepared {
    _directory: tempfile::TempDir,
    pub path: PathBuf,
    pub author: String,
    pub original_bytes: usize,
    pub bytes: usize,
    pub images: usize,
}

/// Keep Unicode while making a single, bounded FAT-compatible component.
pub fn component(value: &str, fallback: &str) -> String {
    let mut result = String::new();
    for c in value.chars() {
        let c = if c.is_control() || "<>:\"/\\|?*".contains(c) {
            '_'
        } else {
            c
        };
        if result.len() + c.len_utf8() > 120 {
            break;
        }
        result.push(c);
    }
    let result = result.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if result.is_empty()
        || ["system volume information", "xtcache"].contains(&result.to_lowercase().as_str())
    {
        fallback.to_owned()
    } else {
        result.to_owned()
    }
}

pub fn prepare(book: &Path, optimize: bool, organized: bool) -> Result<Prepared> {
    let file = fs::File::open(book)?;
    ensure!(file.metadata()?.is_file(), "Expected a regular EPUB file");
    let mut data = Vec::new();
    file.take(epub::MAX_BOOK_BYTES + 1).read_to_end(&mut data)?;
    ensure!(
        data.len() as u64 <= epub::MAX_BOOK_BYTES,
        "EPUB exceeds 128 MiB"
    );
    epub::validate(&data)?;
    epub::font_metadata(&data)?;
    let metadata = epub::metadata(book)?.context("Missing EPUB metadata")?;
    let author = component(&metadata.author, "Unknown author");
    let stem = book
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled");
    let name = if organized {
        format!(
            "{}.epub",
            component(metadata.title.as_deref().unwrap_or(stem), "Untitled")
        )
    } else {
        book.file_name()
            .and_then(|s| s.to_str())
            .context("Invalid EPUB filename")?
            .to_owned()
    };
    let original_bytes = data.len();
    let (data, images) = if optimize {
        optimize_images(&data)?
    } else {
        (data, 0)
    };
    let directory = tempfile::tempdir()?;
    let path = directory.path().join(name);
    fs::write(&path, &data)?;
    Ok(Prepared {
        _directory: directory,
        path,
        author,
        original_bytes,
        bytes: data.len(),
        images,
    })
}

/// Match the X4 screen dimensions. Keep resource names and media types so all
/// OPF, CSS, SVG and XHTML references remain intact. Never crop or remove content.
fn optimize_images(data: &[u8]) -> Result<(Vec<u8>, usize)> {
    let mut archive = ZipArchive::new(Cursor::new(data))?;
    const MARKER: &str = "META-INF/xteink-device-profile.txt";
    const PROFILE: &[u8] = b"xteink-x4-images-v1:480x800:gray:jpeg85";
    if let Ok(mut marker) = archive.by_name(MARKER) {
        let mut value = Vec::new();
        marker.by_ref().take(256).read_to_end(&mut value)?;
        if value == PROFILE {
            return Ok((data.to_vec(), 0));
        }
    }
    let mut out = ZipWriter::new(Cursor::new(Vec::new()));
    out.start_file(
        "mimetype",
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )?;
    out.write_all(b"application/epub+zip")?;
    let mut images = 0;
    for index in 0..archive.len() {
        let mut member = archive.by_index(index)?;
        if member.name() == "mimetype" || member.name() == MARKER {
            continue;
        }
        let name = member.name().to_owned();
        let mut bytes = Vec::new();
        member.read_to_end(&mut bytes)?;
        if !member.is_dir() {
            let format = image::guess_format(&bytes).ok();
            // Leave animated PNGs intact rather than silently discarding frames.
            let animated_png =
                format == Some(ImageFormat::Png) && bytes.windows(4).any(|v| v == b"acTL");
            if matches!(format, Some(ImageFormat::Jpeg | ImageFormat::Png)) && !animated_png {
                let format = format.unwrap();
                let mut reader = ImageReader::with_format(Cursor::new(&bytes), format);
                let mut limits = image::Limits::default();
                limits.max_image_width = Some(16384);
                limits.max_image_height = Some(16384);
                limits.max_alloc = Some(256 * 1024 * 1024);
                reader.limits(limits);
                let img = reader.decode().with_context(|| format!("Cannot optimize image {name}; use --no-optimize to preserve its original bytes"))?;
                let img = if img.width() > 480 || img.height() > 800 {
                    img.resize(480, 800, FilterType::Lanczos3)
                } else {
                    img
                };
                let img = img.grayscale();
                let mut encoded = Cursor::new(Vec::new());
                if format == ImageFormat::Jpeg {
                    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 85)
                        .encode_image(&img)?;
                } else {
                    img.write_to(&mut encoded, ImageFormat::Png)?;
                }
                bytes = encoded.into_inner();
                images += 1;
            }
        }
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        if member.is_dir() {
            out.add_directory(name, options)?;
        } else {
            out.start_file(name, options)?;
            out.write_all(&bytes)?;
        }
    }
    if images == 0 {
        return Ok((data.to_vec(), 0));
    }
    out.start_file(
        MARKER,
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )?;
    out.write_all(PROFILE)?;
    let bytes = out.finish()?.into_inner();
    epub::validate(&bytes).context("Optimized EPUB failed validation")?;
    ensure!(
        bytes.len() as u64 <= epub::MAX_BOOK_BYTES,
        "Optimized EPUB exceeds 128 MiB"
    );
    Ok((bytes, images))
}

/// Only create the author directory beneath an already-mounted destination.
pub fn author_directory(destination: &Path, author: &str) -> Result<PathBuf> {
    ensure!(
        destination.is_dir(),
        "Destination must be an existing mounted-card directory"
    );
    ensure!(
        component(author, "Unknown author") == author,
        "Invalid author directory"
    );
    for entry in fs::read_dir(destination)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().to_lowercase() == author.to_lowercase() {
            ensure!(
                entry.file_type()?.is_dir(),
                "Author directory is occupied by a file or symlink"
            );
            return Ok(entry.path());
        }
    }
    let path = destination.join(author);
    fs::create_dir(&path)?;
    Ok(path)
}
