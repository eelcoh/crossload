use std::collections::HashSet;
use std::io::{Cursor, Read, Write};

use anyhow::{bail, ensure, Context, Result};
use zip::{write::SimpleFileOptions, ZipArchive, ZipWriter};

pub const MAX_BOOK_BYTES: u64 = 128 * 1024 * 1024;
const MAX_EXPANDED_BYTES: u64 = 256 * 1024 * 1024;
const ENCRYPTION: &str = "META-INF/encryption.xml";

pub struct Metadata {
    pub title: Option<String>,
    pub author: String,
    pub identifiers: Vec<String>,
    /// The series this book belongs to, and where it sits in it.
    pub series: Option<String>,
    pub series_index: Option<f32>,
}

/// Identify EPUBs by their contents, including Calibre files without extensions.
/// Read only bounded metadata here; full validation belongs to import.
pub fn metadata(path: &std::path::Path) -> Result<Option<Metadata>> {
    use std::io::{Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let mut magic = [0; 4];
    if file.read(&mut magic)? != 4 || magic != *b"PK\x03\x04" {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(0))?;
    let mut archive = ZipArchive::new(file)?;
    if archive.index_for_name("META-INF/container.xml").is_none()
        || archive.index_for_name("mimetype").is_none()
    {
        return Ok(None);
    }
    let mut member = |name: &str| -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        archive
            .by_name(name)?
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= 8 * 1024 * 1024,
            "EPUB metadata exceeds 8 MiB"
        );
        Ok(bytes)
    };
    if member("mimetype")? != b"application/epub+zip" {
        return Ok(None);
    }
    let container = member("META-INF/container.xml")?;
    let container = roxmltree::Document::parse(std::str::from_utf8(&container)?)?;
    let root = container
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "rootfile")
        .and_then(|n| n.attribute("full-path"))
        .context("EPUB has no package document")?;
    let package = member(root)?;
    let package = roxmltree::Document::parse(std::str::from_utf8(&package)?)?;
    let dc = "http://purl.org/dc/elements/1.1/";
    let values = |tag| {
        package
            .descendants()
            .filter(move |n| n.has_tag_name((dc, tag)))
            .filter_map(|n| n.text())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    // A series is written one way by EPUB 2 and another by EPUB 3, and a file
    // may carry either. Calibre writes the first; the second refines a named
    // collection with the position in it.
    let metas: Vec<_> = package
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "meta")
        .collect();
    let named = |name: &str| {
        metas
            .iter()
            .find(|n| n.attribute("name") == Some(name))
            .and_then(|n| n.attribute("content"))
    };
    let collection = metas
        .iter()
        .find(|n| n.attribute("property") == Some("belongs-to-collection"));
    let text = |value: &str| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    };
    let series = named("calibre:series")
        .and_then(text)
        .or_else(|| collection.and_then(|n| n.text()).and_then(text));
    let series_index = named("calibre:series_index")
        .and_then(|value| value.trim().parse().ok())
        .or_else(|| {
            let id = format!("#{}", collection?.attribute("id")?);
            metas
                .iter()
                .find(|n| {
                    n.attribute("refines") == Some(id.as_str())
                        && n.attribute("property") == Some("group-position")
                })?
                .text()?
                .trim()
                .parse()
                .ok()
        });
    Ok(Some(Metadata {
        title: values("title").into_iter().next(),
        author: values("creator").join(", "),
        identifiers: values("identifier"),
        series,
        series_index,
    }))
}

/// Check archive limits and CRCs before handing bytes to the upstream decoder.
/// Shared by EPUB and CBZ: the hardening is about ZIP, not about either one.
pub fn inspect(data: &[u8]) -> Result<()> {
    let mut archive = ZipArchive::new(Cursor::new(data)).context("Book is not a ZIP/EPUB")?;
    ensure!(archive.len() <= 20_000, "Archive has too many entries");
    let mut names = HashSet::new();
    let mut total = 0_u64;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        ensure!(
            names.insert(file.name().to_owned()),
            "Duplicate archive entry: {}",
            file.name()
        );
        ensure!(file.enclosed_name().is_some(), "Invalid archive entry path");
        ensure!(!file.name().contains('\\'), "Invalid archive entry path");
        total = total
            .checked_add(file.size())
            .context("Archive size overflow")?;
        ensure!(
            total <= MAX_EXPANDED_BYTES,
            "Expanded archive exceeds 256 MiB"
        );
        let actual = std::io::copy(
            &mut file.by_ref().take(MAX_EXPANDED_BYTES + 1),
            &mut std::io::sink(),
        )?;
        ensure!(actual == file.size(), "Archive entry size mismatch");
    }
    Ok(())
}

/// Kobo's key table is separate from EPUB encryption metadata. Preserve font
/// obfuscation metadata that Flamberge would otherwise discard, and reject
/// additional DRM schemes rather than silently producing an unusable book.
pub fn font_metadata(data: &[u8]) -> Result<Option<Vec<u8>>> {
    let mut archive = ZipArchive::new(Cursor::new(data))?;
    ensure!(
        archive.index_for_name("META-INF/rights.xml").is_none(),
        "This book contains Adobe rights metadata; Adobe import is not implemented yet"
    );
    if archive.index_for_name(ENCRYPTION).is_none() {
        return Ok(None);
    }
    let bytes = read(&mut archive, ENCRYPTION)?;
    let text = std::str::from_utf8(&bytes).context("Invalid encryption metadata encoding")?;
    let doc = roxmltree::Document::parse(text).context("Invalid encryption metadata")?;
    for entry in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "EncryptedData")
    {
        let algorithm = entry
            .descendants()
            .find(|n| n.is_element() && n.tag_name().name() == "EncryptionMethod")
            .and_then(|n| n.attribute("Algorithm"));
        ensure!(
            matches!(
                algorithm,
                Some("http://www.idpf.org/2008/embedding" | "http://ns.adobe.com/pdf/enc#RC")
            ),
            "EPUB uses an unsupported encryption method"
        );
    }
    Ok(Some(bytes))
}

pub fn restore_font_metadata(data: Vec<u8>, metadata: Option<&[u8]>) -> Result<Vec<u8>> {
    let Some(metadata) = metadata else {
        return Ok(data);
    };
    let mut writer = ZipWriter::new_append(Cursor::new(data))?;
    writer.start_file(ENCRYPTION, SimpleFileOptions::default())?;
    writer.write_all(metadata)?;
    Ok(writer.finish()?.into_inner())
}

/// Basic structural and reading-order validation; this is not full EPUBCheck.
pub fn validate(data: &[u8]) -> Result<()> {
    inspect(data)?;
    let mut archive = ZipArchive::new(Cursor::new(data))?;
    ensure!(
        read(&mut archive, "mimetype")? == b"application/epub+zip",
        "Invalid EPUB mimetype"
    );
    let container = read(&mut archive, "META-INF/container.xml")?;
    let doc = roxmltree::Document::parse(std::str::from_utf8(&container)?)
        .context("Invalid EPUB container.xml")?;
    let root = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "rootfile")
        .and_then(|n| n.attribute("full-path"))
        .context("EPUB has no package document")?;
    let package = read(&mut archive, root).context("EPUB package document is missing")?;
    let package = roxmltree::Document::parse(std::str::from_utf8(&package)?)
        .context("Invalid EPUB package document")?;
    let base = root
        .rsplit_once('/')
        .map(|(base, _)| format!("{base}/"))
        .unwrap_or_default();
    let mut count = 0;
    for itemref in package
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "itemref")
    {
        let id = itemref
            .attribute("idref")
            .context("Spine entry has no idref")?;
        let item = package
            .descendants()
            .find(|n| {
                n.is_element() && n.tag_name().name() == "item" && n.attribute("id") == Some(id)
            })
            .context("Spine references a missing manifest item")?;
        let href = item
            .attribute("href")
            .context("Manifest item has no href")?;
        let path = resolve_href(&base, href)?;
        let chapter =
            read(&mut archive, &path).with_context(|| format!("Missing chapter {path}"))?;
        if item.attribute("media-type") == Some("application/xhtml+xml") {
            roxmltree::Document::parse_with_options(
                std::str::from_utf8(&chapter).context("Chapter is not UTF-8")?,
                roxmltree::ParsingOptions {
                    allow_dtd: true,
                    ..Default::default()
                },
            )
            .with_context(|| format!("Chapter {path} is not readable XHTML"))?;
        }
        count += 1;
    }
    ensure!(count > 0, "EPUB has no reading order");
    Ok(())
}

fn resolve_href(base: &str, href: &str) -> Result<String> {
    let href = href.split('#').next().unwrap_or(href);
    let mut decoded = Vec::new();
    let mut bytes = href.as_bytes().iter().copied();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let a = bytes.next().context("Invalid escaped EPUB path")?;
            let b = bytes.next().context("Invalid escaped EPUB path")?;
            let pair = [a, b];
            decoded.push(u8::from_str_radix(std::str::from_utf8(&pair)?, 16)?);
        } else {
            decoded.push(byte);
        }
    }
    let decoded = String::from_utf8(decoded)?;
    ensure!(
        !decoded.starts_with('/') && !decoded.contains(['\\', ':']),
        "Invalid chapter path"
    );
    let joined = format!("{base}{decoded}");
    let mut parts = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    bail!("Chapter path leaves EPUB");
                }
            }
            other => parts.push(other),
        }
    }
    Ok(parts.join("/"))
}

fn read(archive: &mut ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    archive
        .by_name(name)
        .with_context(|| format!("Missing EPUB entry {name}"))?
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}
