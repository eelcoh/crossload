//! A mounted Kobo: its book files through the `Dir` for its mount, and what
//! only its database knows through the `Kobo` capability.
use std::sync::Arc;

use ambit_caps_dir::{DirErrorKind, DirWrite, RelPath};
use anyhow::{ensure, Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub use crossload_kobo::{Annotation, Kobo as Database, Shelf, Shelved, StoreBook};

use crate::{epub, files, format, format::Format};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    KoboStore,
    Sideloaded,
}

#[derive(Clone, Debug, Serialize)]
pub struct Book {
    pub id: String,
    pub title: String,
    pub author: String,
    /// Store books are always EPUB; a sideloaded file is whatever was put there.
    #[serde(default)]
    pub format: Format,
    pub encrypted: bool,
    pub preview: bool,
    pub source: Source,
    /// Device-relative path for sideloaded entries; also distinguishes copies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip)]
    kobo_volume_id: Option<String>,
}

/// A reader's marks on one book, as Markdown.
///
/// Quoted rather than reproduced as body text, so it stays clear on the page
/// which words are the book's and which are the reader's.
pub fn as_markdown(title: &str, author: &str, notes: &[&Annotation]) -> String {
    let mut out = format!("# {title}\n");
    if !author.is_empty() {
        out.push_str(&format!("\n*{author}*\n"));
    }
    for note in notes {
        out.push('\n');
        if !note.text.is_empty() {
            for line in note.text.lines() {
                out.push_str(&format!("> {}\n", line.trim()));
            }
        }
        if !note.note.is_empty() {
            if !note.text.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("{}\n", note.note));
        }
        // The date the device recorded, trimmed to the day it happened on.
        let day = note.created.split('T').next().unwrap_or("").trim();
        let at = (note.progress * 100.0).round() as i64;
        out.push_str(&match (day.is_empty(), at) {
            (true, _) => format!("\n— {at}% in\n"),
            (false, _) => format!("\n— {at}% in, {day}\n"),
        });
    }
    out
}

/// A mounted Kobo.
#[derive(Clone)]
pub struct Device {
    /// The mount: books are read from it, and copies to the Kobo are added to it.
    pub dir: Arc<dyn DirWrite>,
    pub db: Arc<dyn Database>,
    /// Where it is mounted, for messages and to tell devices apart.
    pub display: String,
}

fn database_error(e: crossload_kobo::KoboError) -> anyhow::Error {
    anyhow::anyhow!(e.message)
}

impl Device {
    /// Every book on the device: store books whose file is in `.kobo/kepub`,
    /// and every sideloaded file that is a book.
    pub fn books(&self) -> Result<Vec<Book>> {
        files::connected(&*self.dir, "The Kobo")?;
        // Chapter records, sideloaded books, and cloud-only entries have no
        // corresponding direct child in .kobo/kepub.
        let mut books: Vec<Book> = self
            .db
            .books()
            .map_err(database_error)?
            .into_iter()
            .filter(|book| {
                book_path(&book.id)
                    .and_then(|path| Ok(self.dir.metadata(&path)?))
                    .is_ok_and(|meta| !meta.is_dir)
            })
            .map(|book| Book {
                id: book.id,
                title: book.title,
                author: book.author,
                format: Format::Epub,
                encrypted: book.encrypted,
                preview: book.preview,
                source: Source::KoboStore,
                path: None,
                kobo_volume_id: None,
            })
            .collect();
        self.sideloaded_books(&RelPath::root(), 0, &mut books)?;
        books.sort_by(|a, b| {
            a.title
                .to_lowercase()
                .cmp(&b.title.to_lowercase())
                .then(a.id.cmp(&b.id))
        });
        Ok(books)
    }

    pub fn shelves(&self) -> Result<Vec<Shelf>> {
        self.db.shelves().map_err(database_error)
    }

    /// A highlight or a note, with the book it was taken from. Nothing about it
    /// is written back: a Kobo's database is the device's, not ours.
    pub fn annotations(&self) -> Result<Vec<Annotation>> {
        self.db.annotations().map_err(database_error)
    }

    /// The device file a book is imported from. Discovery fingerprints it to
    /// detect change without decrypting the book again.
    pub fn source_path(&self, book: &Book) -> Result<RelPath> {
        match &book.path {
            Some(relative) => files::rel(relative),
            None => book_path(&book.id),
        }
    }

    /// The book as an EPUB (or the PDF or CBZ it is), decrypted when the store
    /// encrypted it, with the name an import gives it.
    pub fn import(&self, id: &str, serial: Option<&str>) -> Result<(String, Vec<u8>)> {
        let book = self
            .books()?
            .into_iter()
            .find(|b| b.id == id)
            .context("Book ID is not downloaded on this Kobo; use `crossload kobo list`")?;
        ensure!(
            !book.preview,
            "This Kobo entry is a preview, not the full book. Download the full edition on your Kobo, then reconnect and retry"
        );
        let data = files::book(&*self.dir, &self.source_path(&book)?)?;
        if !book.format.rewritten() {
            // Nothing on a Kobo encrypts a sideloaded PDF or CBZ, and nothing
            // here rewrites one: it travels exactly as it sits on the device.
            format::validate(book.format, &data)?;
            return Ok((output_name(&book), data));
        }
        epub::inspect(&data)?;
        let fonts = epub::font_metadata(&data)?;
        let data = if book.encrypted {
            let volume_id = book.kobo_volume_id.as_deref().unwrap_or(id);
            self.check_encrypted_members(volume_id, &data)?;
            let serial = match serial {
                Some(value) => value.trim().to_string(),
                None => self.device_serial()?,
            };
            ensure!(!serial.is_empty(), "Kobo serial must not be empty");
            let plain = self
                .db
                .decrypt(volume_id, &data, &serial)
                .map_err(database_error)?;
            epub::restore_font_metadata(plain, fonts.as_deref())?
        } else {
            data
        };
        epub::validate(&data)
            .context("Imported book failed EPUB validation; no output was written")?;
        Ok((output_name(&book), data))
    }

    fn sideloaded_books(
        &self,
        directory: &RelPath,
        depth: usize,
        books: &mut Vec<Book>,
    ) -> Result<()> {
        ensure!(depth <= 32, "Kobo folder tree exceeds 32 levels");
        for entry in self.dir.list(directory)? {
            let name = entry.name;
            if name.starts_with('.') || (directory.is_root() && name == "fonts") {
                continue;
            }
            let path = directory.join(&name)?;
            if entry.is_dir {
                self.sideloaded_books(&path, depth + 1, books)?;
                continue;
            }
            // A link that leads off the device is not the device's: skipped, as
            // links always were here.
            let file = match self.dir.open(&path) {
                Err(e) if e.kind == DirErrorKind::Denied => continue,
                file => file,
            };
            let metadata = file
                .map_err(anyhow::Error::from)
                .and_then(epub::metadata)
                .with_context(|| format!("Cannot read sideloaded book metadata: {path}"))?;
            // An EPUB is recognized by its contents, so Calibre files without
            // an extension still count; the others have only their name to go
            // on.
            let book_format = match &metadata {
                Some(_) => Format::Epub,
                None => match Format::of(&name) {
                    Some(found) if found != Format::Epub => found,
                    _ => continue,
                },
            };
            let stable_path = path.to_string();
            let hash = format!("{:x}", Sha256::digest(stable_path.as_bytes()));
            let kobo_volume_id = match &metadata {
                Some(metadata) => self.match_encrypted_store_copy(&path, &metadata.identifiers)?,
                None => None,
            };
            books.push(Book {
                id: format!("file:{}", &hash[..24]),
                title: metadata
                    .as_ref()
                    .and_then(|m| m.title.clone())
                    .unwrap_or_else(|| format::name_title(&name)),
                author: metadata
                    .as_ref()
                    .map(|m| m.author.clone())
                    .unwrap_or_default(),
                format: book_format,
                encrypted: kobo_volume_id.is_some(),
                preview: false,
                source: Source::Sideloaded,
                path: Some(stable_path),
                kobo_volume_id,
            });
        }
        Ok(())
    }

    fn match_encrypted_store_copy(
        &self,
        path: &RelPath,
        identifiers: &[String],
    ) -> Result<Option<String>> {
        for identifier in identifiers {
            let id = identifier.strip_prefix("urn:uuid:").unwrap_or(identifier);
            if self
                .db
                .encrypted_members(id)
                .map_err(database_error)?
                .is_empty()
            {
                continue;
            }
            let Ok(original) = book_path(id) else {
                continue;
            };
            let Ok(original_meta) = self.dir.metadata(&original) else {
                continue;
            };
            // An identifier alone is insufficient: an already-decrypted copy
            // retains the same identifier. Only reuse keys for identical files.
            if self.dir.metadata(path)?.len == original_meta.len
                && digest(&*self.dir, path)? == digest(&*self.dir, &original)?
            {
                return Ok(Some(id.to_owned()));
            }
        }
        Ok(None)
    }

    fn device_serial(&self) -> Result<String> {
        let xml = files::read(
            &*self.dir,
            &RelPath::new([".adobe-digital-editions", "device.xml"])?,
            1 << 20,
        )
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .context("Cannot read Kobo device.xml; supply --serial with the Kobo device serial")?;
        let document =
            roxmltree::Document::parse(&xml).context("Invalid Kobo device.xml; supply --serial")?;
        document
            .descendants()
            .find(|node| node.is_element() && node.tag_name().name() == "deviceSerial")
            .and_then(|node| node.text())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .context("No serial in device.xml; supply --serial")
    }

    fn check_encrypted_members(&self, id: &str, data: &[u8]) -> Result<()> {
        let archive = zip::ZipArchive::new(std::io::Cursor::new(data))?;
        for name in self.db.encrypted_members(id).map_err(database_error)? {
            ensure!(
                archive.index_for_name(&name).is_some(),
                "Downloaded book is missing encrypted member {name}"
            );
        }
        Ok(())
    }
}

/// A store book's file: `.kobo/kepub/<id>`, where the ID must be one plain name.
fn book_path(id: &str) -> Result<RelPath> {
    RelPath::new([".kobo", "kepub", id]).context("Invalid Kobo book ID")
}

fn digest(dir: &dyn ambit_caps_dir::Dir, path: &RelPath) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut file = dir.open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 16 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hash.finalize().to_vec())
}

/// Write an imported book into the import folder, never over an existing file.
/// With `reuse`, an identical file already there is the import.
pub fn publish(output: &dyn DirWrite, name: &str, data: &[u8], reuse: bool) -> Result<RelPath> {
    let target = RelPath::new([name])?;
    if reuse {
        match output.metadata(&target) {
            Ok(meta) => {
                ensure!(!meta.is_dir && meta.len == data.len() as u64 && files::book(output, &target)? == data,
                    "Existing local import differs or is not a regular file; nothing was overwritten: {target}");
                return Ok(target);
            }
            Err(e) if e.kind == DirErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    output.create_new(&target, data).map_err(|e| match e.kind {
        DirErrorKind::Exists => {
            anyhow::anyhow!("Cannot save {target}; existing files are never overwritten")
        }
        _ => anyhow::Error::new(e).context(format!("Cannot save {target}")),
    })?;
    Ok(target)
}

fn output_name(book: &Book) -> String {
    let mut title: String = book
        .title
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .take(80)
        .collect();
    while title.len() > 180 {
        title.pop();
    }
    let title = title.trim_matches([' ', '.']);
    let title = if title.is_empty() { "Book" } else { title };
    let hash = format!("{:x}", Sha256::digest(book.id.as_bytes()));
    format!("{} [{}].{}", title, &hash[..12], book.format.extension())
}
