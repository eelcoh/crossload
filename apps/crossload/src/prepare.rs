//! Device copies: bounded image decoding, stable EPUB resources and safe names.
//! A format Crossload does not rewrite is named and carried through untouched.
use crate::{epub, files, format, format::Format, profile::Profile};
use ambit_caps_dir::{DirWrite, RelPath};
use anyhow::{ensure, Context, Result};
use image::{imageops::FilterType, ImageFormat, ImageReader};
use std::io::{Cursor, Read, Write};
use zip::{write::SimpleFileOptions, ZipArchive, ZipWriter};

/// A device copy, in memory: what goes onto the device, and under which name.
pub struct Prepared {
    pub name: String,
    pub data: Vec<u8>,
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

/// `optimize` names the screen to prepare for, or is None to leave a book's
/// images exactly as they are. `name` is the book's file name, which decides
/// its format and, unless `organized`, the name of the copy.
pub fn prepare(
    data: Vec<u8>,
    name: &str,
    optimize: Option<Profile>,
    organized: bool,
) -> Result<Prepared> {
    let kind = Format::of(name).unwrap_or_default();
    ensure!(
        data.len() as u64 <= epub::MAX_BOOK_BYTES,
        "Book exceeds 128 MiB"
    );
    format::validate(kind, &data)?;
    let metadata = if kind.rewritten() {
        Some(epub::metadata_from(&data)?.context("Missing EPUB metadata")?)
    } else {
        None
    };
    let author = component(
        metadata.as_ref().map_or("", |m| m.author.as_str()),
        "Unknown author",
    );
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    let name = if organized {
        let title = metadata
            .as_ref()
            .and_then(|m| m.title.as_deref())
            .unwrap_or(stem);
        format!("{}.{}", component(title, "Untitled"), kind.extension())
    } else {
        ensure!(!name.is_empty(), "Invalid book filename");
        name.to_owned()
    };
    let original_bytes = data.len();
    // Optimization is an EPUB rewrite; there is nothing to rewrite otherwise.
    let (data, images) = match optimize.filter(|_| kind.rewritten()) {
        Some(profile) => optimize_images(&data, profile)?,
        None => (data, 0),
    };
    Ok(Prepared {
        name,
        bytes: data.len(),
        data,
        author,
        original_bytes,
        images,
    })
}

/// Fit the profile's screen. Keep resource names and media types so all
/// OPF, CSS, SVG and XHTML references remain intact. Never crop or remove content.
fn optimize_images(data: &[u8], profile: Profile) -> Result<(Vec<u8>, usize)> {
    let mut archive = ZipArchive::new(Cursor::new(data))?;
    // The marker's name is frozen. Every device copy already on a reader or a
    // card carries it, and renaming it would make those copies read as
    // originals: the ◐ would vanish and they would stop grouping with the books
    // they came from. What it holds is the screen the copy was made for, so a
    // copy made for one device is not mistaken for a copy made for another.
    const MARKER: &str = "META-INF/xteink-device-profile.txt";
    if let Ok(mut marker) = archive.by_name(MARKER) {
        let mut value = Vec::new();
        marker.by_ref().take(256).read_to_end(&mut value)?;
        if value == profile.marker.as_bytes() {
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
                let img = if img.width() > profile.width || img.height() > profile.height {
                    img.resize(profile.width, profile.height, FilterType::Lanczos3)
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
    out.write_all(profile.marker.as_bytes())?;
    let bytes = out.finish()?.into_inner();
    epub::validate(&bytes).context("Optimized EPUB failed validation")?;
    ensure!(
        bytes.len() as u64 <= epub::MAX_BOOK_BYTES,
        "Optimized EPUB exceeds 128 MiB"
    );
    Ok((bytes, images))
}

/// The author's folder inside `destination`, created inside an existing
/// folder only, and found again whatever case it was created in.
pub fn author_directory(
    destination: &dyn DirWrite,
    folder: &RelPath,
    author: &str,
) -> Result<RelPath> {
    files::connected(destination, "The destination")?;
    ensure!(
        destination.metadata(folder).is_ok_and(|m| m.is_dir),
        "Destination must be an existing mounted-card directory"
    );
    ensure!(
        component(author, "Unknown author") == author,
        "Invalid author directory"
    );
    if let Some(entry) = files::find_folded(destination, folder, author)? {
        ensure!(
            entry.is_dir,
            "Author directory is occupied by a file or symlink"
        );
        return Ok(folder.join(&entry.name)?);
    }
    let path = folder.join(author)?;
    destination.create_dir(&path)?;
    Ok(path)
}
