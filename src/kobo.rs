use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use flamberge_schemes::KeyStore;
use rusqlite::{backup::Backup, Connection, OpenFlags};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::epub;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    KoboStore,
    Sideloaded,
}

#[derive(Debug, Serialize)]
pub struct Book {
    pub id: String,
    pub title: String,
    pub author: String,
    pub encrypted: bool,
    pub preview: bool,
    pub source: Source,
    /// Device-relative path for sideloaded entries; also distinguishes copies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(skip)]
    kobo_volume_id: Option<String>,
}

pub struct Library {
    root: PathBuf,
    book_dir: PathBuf,
    db: Connection,
    db_bytes: Vec<u8>,
}

impl Library {
    pub fn open(root: &Path) -> Result<Self> {
        let root = root
            .canonicalize()
            .with_context(|| format!("Cannot open Kobo at {}", root.display()))?;
        let database = root.join(".kobo/KoboReader.sqlite");
        ensure!(
            database.is_file(),
            "No .kobo/KoboReader.sqlite found; pass the Kobo mount directory with --device"
        );
        ensure!(
            database.canonicalize()?.starts_with(&root),
            "Kobo database points outside the device"
        );
        let book_dir = root.join(".kobo/kepub");
        let book_dir = if book_dir.exists() {
            book_dir.canonicalize()?
        } else {
            book_dir
        };
        ensure!(
            book_dir.starts_with(&root),
            "Kobo book directory points outside the device"
        );
        let db_bytes = snapshot(&database)?;
        let file = NamedTempFile::new()?;
        fs::write(file.path(), &db_bytes)?;
        let source = Connection::open(file.path())?;
        let mut db = Connection::open_in_memory()?;
        Backup::new(&source, &mut db)?.run_to_completion(128, Duration::from_millis(10), None)?;
        Ok(Self {
            root,
            book_dir,
            db,
            db_bytes,
        })
    }

    pub fn books(&self) -> Result<Vec<Book>> {
        // Kobo uses Accessibility=6 for previews (also mapped by Calibre's
        // Kobo driver). A local file and IsDownloaded=true do not imply that
        // the full edition is present. Keep older/minimal schemas readable.
        let has_accessibility: bool = self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('content') WHERE lower(name) = 'accessibility')",
            [],
            |row| row.get(0),
        )?;
        let preview = if has_accessibility {
            "COALESCE(CAST(Accessibility AS INTEGER) = 6, 0)"
        } else {
            "0"
        };
        let mut query = self
            .db
            .prepare(&format!(
                "SELECT ContentID, COALESCE(Title, ContentID), COALESCE(Attribution, ''),
             EXISTS(SELECT 1 FROM content_keys WHERE volumeid = content.ContentID), {preview}
             FROM content ORDER BY Title COLLATE NOCASE, ContentID"
            ))
            .context("Unsupported Kobo database schema")?;
        let rows = query.query_map([], |row| {
            Ok(Book {
                id: row.get(0)?,
                title: row.get(1)?,
                author: row.get(2)?,
                encrypted: row.get(3)?,
                preview: row.get(4)?,
                source: Source::KoboStore,
                path: None,
                kobo_volume_id: None,
            })
        })?;
        let mut books = Vec::new();
        for row in rows {
            let book = row?;
            // Chapter records, sideloaded books, and cloud-only entries have no
            // corresponding direct child in .kobo/kepub.
            if let Ok(path) = self.book_path(&book.id) {
                if path.is_file() {
                    books.push(book);
                }
            }
        }
        self.sideloaded_books(&self.root, &mut books)?;
        books.sort_by(|a, b| {
            a.title
                .to_lowercase()
                .cmp(&b.title.to_lowercase())
                .then(a.id.cmp(&b.id))
        });
        Ok(books)
    }

    pub fn import(&self, id: &str, output: &Path, serial: Option<&str>) -> Result<PathBuf> {
        let book = self
            .books()?
            .into_iter()
            .find(|b| b.id == id)
            .context("Book ID is not downloaded on this Kobo; use `crossload kobo list`")?;
        ensure!(
            !book.preview,
            "This Kobo entry is a preview, not the full book. Download the full edition on your Kobo, then reconnect and retry"
        );
        let path = match &book.path {
            Some(relative) => {
                let path = self.root.join(relative).canonicalize()?;
                ensure!(
                    path.starts_with(&self.root),
                    "Book path points outside the device"
                );
                path
            }
            None => self.book_path(id)?,
        };
        let mut data = Vec::new();
        fs::File::open(&path)?
            .take(epub::MAX_BOOK_BYTES + 1)
            .read_to_end(&mut data)?;
        ensure!(
            data.len() as u64 <= epub::MAX_BOOK_BYTES,
            "Book exceeds 128 MiB"
        );
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
            let mut query = self.db.prepare("SELECT UserID FROM user")?;
            let users = query
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ensure!(!users.is_empty(), "Kobo database has no account IDs");
            let mut keys = KeyStore::new();
            keys.kobo_keys = flamberge_keys::kobo::derive_userkeys(&[serial], &users);
            keys.kobo_db = Some(self.db_bytes.clone());
            keys.kobo_volumeid = Some(volume_id.to_owned());
            let result = flamberge_schemes::kobo::decrypt(&data, &keys)
                .context("Kobo import failed; check the device serial and that this book is fully downloaded")?;
            epub::restore_font_metadata(result.data, fonts.as_deref())?
        } else {
            data
        };
        epub::validate(&data)
            .context("Imported book failed EPUB validation; no output was written")?;
        let output = output_directory(output, &self.root)?;
        let target = output.join(output_name(&book));
        let mut temporary = NamedTempFile::new_in(&output)?;
        temporary.write_all(&data)?;
        temporary.as_file().sync_all()?;
        temporary.persist_noclobber(&target).with_context(|| {
            format!(
                "Cannot save {}; existing files are never overwritten",
                target.display()
            )
        })?;
        Ok(target)
    }

    fn sideloaded_books(&self, directory: &Path, books: &mut Vec<Book>) -> Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.')
                || (directory == self.root && name == "fonts")
            {
                continue;
            }
            let kind = entry.file_type()?;
            // Do not traverse symlinks: no cycles or access outside the device.
            if kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            if kind.is_dir() {
                self.sideloaded_books(&path, books)?;
            } else if kind.is_file() {
                let Some(metadata) = epub::metadata(&path).with_context(|| {
                    format!("Cannot read sideloaded book metadata: {}", path.display())
                })?
                else {
                    continue;
                };
                let relative = path.strip_prefix(&self.root)?;
                let stable_path = relative.to_str().context("Book filename is not UTF-8")?;
                let hash = format!("{:x}", Sha256::digest(stable_path.as_bytes()));
                let kobo_volume_id =
                    self.match_encrypted_store_copy(&path, &metadata.identifiers)?;
                books.push(Book {
                    id: format!("file:{}", &hash[..24]),
                    title: metadata
                        .title
                        .unwrap_or_else(|| name.to_string_lossy().into_owned()),
                    author: metadata.author,
                    encrypted: kobo_volume_id.is_some(),
                    preview: false,
                    source: Source::Sideloaded,
                    path: Some(relative.to_path_buf()),
                    kobo_volume_id,
                });
            }
        }
        Ok(())
    }

    fn match_encrypted_store_copy(
        &self,
        path: &Path,
        identifiers: &[String],
    ) -> Result<Option<String>> {
        for identifier in identifiers {
            let id = identifier.strip_prefix("urn:uuid:").unwrap_or(identifier);
            let encrypted: bool = self.db.query_row(
                "SELECT EXISTS(SELECT 1 FROM content_keys WHERE volumeid = ?1)",
                [id],
                |row| row.get(0),
            )?;
            if !encrypted {
                continue;
            }
            let Ok(original) = self.book_path(id) else {
                continue;
            };
            // An identifier alone is insufficient: an already-decrypted copy
            // retains the same identifier. Only reuse keys for identical files.
            if fs::metadata(path)?.len() == fs::metadata(&original)?.len()
                && file_digest(path)? == file_digest(&original)?
            {
                return Ok(Some(id.to_owned()));
            }
        }
        Ok(None)
    }

    fn book_path(&self, id: &str) -> Result<PathBuf> {
        let mut parts = Path::new(id).components();
        ensure!(
            matches!(parts.next(), Some(Component::Normal(_)))
                && parts.next().is_none()
                && !id.contains(['/', '\\']),
            "Invalid Kobo book ID"
        );
        let path = self.book_dir.join(id).canonicalize()?;
        ensure!(
            path.starts_with(&self.book_dir),
            "Book path points outside Kobo book directory"
        );
        Ok(path)
    }

    fn device_serial(&self) -> Result<String> {
        let path = self.root.join(".adobe-digital-editions/device.xml");
        let xml = fs::read_to_string(path)
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
        let mut query = self
            .db
            .prepare("SELECT elementid FROM content_keys WHERE volumeid = ?1")?;
        let names = query.query_map([id], |r| r.get::<_, String>(0))?;
        for name in names {
            let name = name?;
            ensure!(
                archive.index_for_name(&name).is_some(),
                "Downloaded book is missing encrypted member {name}"
            );
        }
        Ok(())
    }
}

fn file_digest(path: &Path) -> Result<Vec<u8>> {
    let mut file = fs::File::open(path)?;
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

// Copy the main database and its WAL to temporary storage. SQLite recovers the
// copy, then its backup API makes a self-contained snapshot for Flamberge. Never
// patch the source header or ask SQLite to create files on the device.
fn snapshot(database: &Path) -> Result<Vec<u8>> {
    let temp = tempfile::tempdir()?;
    let wal = database.with_file_name("KoboReader.sqlite-wal");
    let before_db = stamp(database)?;
    let before_wal = optional_stamp(&wal)?;
    let copy = temp.path().join("copy.sqlite");
    fs::copy(database, &copy)?;
    if before_wal.is_some() {
        fs::copy(&wal, temp.path().join("copy.sqlite-wal"))?;
    }
    ensure!(
        before_db == stamp(database)? && before_wal == optional_stamp(&wal)?,
        "Kobo database changed while reading; stop syncing and reconnect the device"
    );
    let source = Connection::open_with_flags(&copy, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("Cannot open Kobo database copy")?;
    let target = temp.path().join("snapshot.sqlite");
    let mut destination = Connection::open(&target)?;
    Backup::new(&source, &mut destination)?.run_to_completion(
        128,
        Duration::from_millis(10),
        None,
    )?;
    drop(destination);
    fs::read(target).context("Cannot read Kobo database snapshot")
}

fn stamp(path: &Path) -> Result<(u64, std::time::SystemTime)> {
    let metadata = fs::metadata(path)?;
    Ok((metadata.len(), metadata.modified()?))
}

fn optional_stamp(path: &Path) -> Result<Option<(u64, std::time::SystemTime)>> {
    match fs::metadata(path) {
        Ok(meta) => Ok(Some((meta.len(), meta.modified()?))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn output_directory(output: &Path, device: &Path) -> Result<PathBuf> {
    // Resolve the existing ancestor before creating directories, including any
    // symlinks. Reject parent traversal to keep this check unambiguous.
    ensure!(
        !output.components().any(|c| c == Component::ParentDir),
        "Output path must not contain .."
    );
    let absolute = if output.is_absolute() {
        output.to_path_buf()
    } else {
        std::env::current_dir()?.join(output)
    };
    let ancestor = absolute
        .ancestors()
        .find(|p| p.exists())
        .context("No existing output parent")?;
    ensure!(
        !ancestor.canonicalize()?.starts_with(device),
        "Choose a local output directory outside the Kobo"
    );
    fs::create_dir_all(&absolute)?;
    let output = absolute.canonicalize()?;
    ensure!(
        !output.starts_with(device),
        "Choose a local output directory outside the Kobo"
    );
    Ok(output)
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
    format!("{} [{}].epub", title, &hash[..12])
}
