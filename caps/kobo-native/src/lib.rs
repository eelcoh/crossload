//! [`Kobo`] on a mounted Kobo: its `.kobo/KoboReader.sqlite`, read from a snapshot.
//!
//! The database and its write-ahead log are copied to a private temporary directory, SQLite
//! recovers the copy there, and its backup API makes a self-contained snapshot in memory. Nothing
//! is ever written to the device: the source header is not patched and SQLite is never asked to
//! create files beside it. A snapshot is reused while the database's size and modification time
//! stay the same, and taken again when they change.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use crossload_kobo::{Annotation, Kobo, KoboError, KoboErrorKind, Shelf, Shelved, StoreBook};
use flamberge_schemes::KeyStore;
use rusqlite::{backup::Backup, Connection, OpenFlags};

pub struct NativeKobo {
    root: PathBuf,
    snapshot: Mutex<Option<Snapshot>>,
}

struct Snapshot {
    stamp: Stamp,
    db: Connection,
    bytes: Vec<u8>,
}

type Stamp = ((u64, SystemTime), Option<(u64, SystemTime)>);

fn io(e: impl std::fmt::Display) -> KoboError {
    KoboError::new(KoboErrorKind::Io, e.to_string())
}

fn schema(e: impl std::fmt::Display) -> KoboError {
    KoboError::new(
        KoboErrorKind::Unsupported,
        format!("Unsupported Kobo database schema: {e}"),
    )
}

impl NativeKobo {
    /// The Kobo mounted at `root`. Nothing is read until the first call, so a Kobo that is not
    /// connected yet is not an error here.
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            snapshot: Mutex::new(None),
        }
    }

    fn database(&self) -> Result<PathBuf, KoboError> {
        let root = self.root.canonicalize().map_err(|e| {
            KoboError::new(
                KoboErrorKind::NotFound,
                format!("Cannot open Kobo at {}: {e}", self.root.display()),
            )
        })?;
        let database = root.join(".kobo/KoboReader.sqlite");
        if !database.is_file() {
            return Err(KoboError::new(
                KoboErrorKind::NotFound,
                "No .kobo/KoboReader.sqlite found; pass the Kobo mount directory with --device",
            ));
        }
        if !database.canonicalize().map_err(io)?.starts_with(&root) {
            return Err(KoboError::new(
                KoboErrorKind::NotFound,
                "Kobo database points outside the device",
            ));
        }
        Ok(database)
    }

    /// The current snapshot, taken again if the database changed since the last one.
    fn snapshot(&self) -> Result<MutexGuard<'_, Option<Snapshot>>, KoboError> {
        let database = self.database()?;
        let stamp = (stamp(&database)?, optional_stamp(&wal(&database))?);
        let mut current = self.snapshot.lock().unwrap_or_else(|e| e.into_inner());
        if current.as_ref().is_none_or(|s| s.stamp != stamp) {
            let bytes = snapshot(&database)?;
            let db = open(&bytes)?;
            *current = Some(Snapshot { stamp, db, bytes });
        }
        Ok(current)
    }

    fn with<T>(
        &self,
        read: impl FnOnce(&Snapshot) -> Result<T, KoboError>,
    ) -> Result<T, KoboError> {
        let guard = self.snapshot()?;
        read(guard.as_ref().expect("just taken"))
    }
}

fn wal(database: &Path) -> PathBuf {
    database.with_file_name("KoboReader.sqlite-wal")
}

/// An in-memory database from snapshot bytes.
fn open(bytes: &[u8]) -> Result<Connection, KoboError> {
    let file = tempfile::NamedTempFile::new().map_err(io)?;
    std::fs::write(file.path(), bytes).map_err(io)?;
    let source = Connection::open(file.path()).map_err(io)?;
    let mut db = Connection::open_in_memory().map_err(io)?;
    Backup::new(&source, &mut db)
        .and_then(|b| b.run_to_completion(128, Duration::from_millis(10), None))
        .map_err(io)?;
    Ok(db)
}

// Copy the main database and its WAL to temporary storage. SQLite recovers the copy, then its
// backup API makes a self-contained snapshot for Flamberge. Never patch the source header or ask
// SQLite to create files on the device.
fn snapshot(database: &Path) -> Result<Vec<u8>, KoboError> {
    let temp = tempfile::tempdir().map_err(io)?;
    let wal = wal(database);
    let before_db = stamp(database)?;
    let before_wal = optional_stamp(&wal)?;
    let copy = temp.path().join("copy.sqlite");
    std::fs::copy(database, &copy).map_err(io)?;
    if before_wal.is_some() {
        std::fs::copy(&wal, temp.path().join("copy.sqlite-wal")).map_err(io)?;
    }
    if before_db != stamp(database)? || before_wal != optional_stamp(&wal)? {
        return Err(KoboError::new(
            KoboErrorKind::Changed,
            "Kobo database changed while reading; stop syncing and reconnect the device",
        ));
    }
    let source = Connection::open_with_flags(&copy, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| io(format!("Cannot open Kobo database copy: {e}")))?;
    let target = temp.path().join("snapshot.sqlite");
    let mut destination = Connection::open(&target).map_err(io)?;
    Backup::new(&source, &mut destination)
        .and_then(|b| b.run_to_completion(128, Duration::from_millis(10), None))
        .map_err(io)?;
    drop(destination);
    std::fs::read(target).map_err(|e| io(format!("Cannot read Kobo database snapshot: {e}")))
}

fn stamp(path: &Path) -> Result<(u64, SystemTime), KoboError> {
    let metadata = std::fs::metadata(path).map_err(io)?;
    Ok((metadata.len(), metadata.modified().map_err(io)?))
}

fn optional_stamp(path: &Path) -> Result<Option<(u64, SystemTime)>, KoboError> {
    match std::fs::metadata(path) {
        Ok(meta) => Ok(Some((meta.len(), meta.modified().map_err(io)?))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io(e)),
    }
}

fn exists(db: &Connection, sql: &str) -> Result<bool, KoboError> {
    db.query_row(sql, [], |row| row.get(0)).map_err(schema)
}

impl Kobo for NativeKobo {
    fn books(&self) -> Result<Vec<StoreBook>, KoboError> {
        self.with(|s| {
            // Kobo uses Accessibility=6 for previews (also mapped by Calibre's Kobo driver). A
            // local file and IsDownloaded=true do not imply that the full edition is present.
            // Keep older/minimal schemas readable.
            let preview = if exists(
                &s.db,
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('content') WHERE lower(name) = 'accessibility')",
            )? {
                "COALESCE(CAST(Accessibility AS INTEGER) = 6, 0)"
            } else {
                "0"
            };
            let mut query = s
                .db
                .prepare(&format!(
                    "SELECT ContentID, COALESCE(Title, ContentID), COALESCE(Attribution, ''),
                     EXISTS(SELECT 1 FROM content_keys WHERE volumeid = content.ContentID), {preview}
                     FROM content ORDER BY Title COLLATE NOCASE, ContentID"
                ))
                .map_err(schema)?;
            let rows = query
                .query_map([], |row| {
                    Ok(StoreBook {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        author: row.get(2)?,
                        encrypted: row.get(3)?,
                        preview: row.get(4)?,
                    })
                })
                .map_err(schema)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(schema)
        })
    }

    fn annotations(&self) -> Result<Vec<Annotation>, KoboError> {
        self.with(|s| {
            // A minimal or older database may not have the table at all, which is an empty
            // record rather than a failure.
            if !exists(
                &s.db,
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'Bookmark')",
            )? {
                return Ok(vec![]);
            }
            let mut query = s
                .db
                .prepare(
                    "SELECT COALESCE(c.Title, b.VolumeID), COALESCE(c.Attribution, ''),
                     COALESCE(b.Text, ''), COALESCE(b.Annotation, ''),
                     COALESCE(b.DateCreated, ''), COALESCE(b.ChapterProgress, 0)
                     FROM Bookmark b LEFT JOIN content c ON c.ContentID = b.VolumeID
                     WHERE COALESCE(b.Hidden, 'false') NOT IN ('true', '1')
                     ORDER BY COALESCE(c.Title, b.VolumeID) COLLATE NOCASE,
                     COALESCE(b.ChapterProgress, 0)",
                )
                .map_err(schema)?;
            let rows = query
                .query_map([], |row| {
                    Ok(Annotation {
                        title: row.get(0)?,
                        author: row.get(1)?,
                        text: row.get::<_, String>(2)?.trim().to_owned(),
                        note: row.get::<_, String>(3)?.trim().to_owned(),
                        created: row.get(4)?,
                        progress: row.get(5)?,
                    })
                })
                .map_err(schema)?;
            Ok(rows
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(schema)?
                .into_iter()
                // A dog-ear marks a page and says nothing; there is nothing in it to carry off
                // the device.
                .filter(|note| !note.text.is_empty() || !note.note.is_empty())
                .collect())
        })
    }

    fn shelves(&self) -> Result<Vec<Shelf>, KoboError> {
        self.with(|s| {
            // A device with no collections may not have the tables at all, which is a device
            // with no collections rather than a failure.
            if !exists(
                &s.db,
                "SELECT (SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'
                 AND name IN ('Shelf', 'ShelfContent')) = 2",
            )? {
                return Ok(vec![]);
            }
            // Shelf rows are keyed by InternalName, which equals Name for a collection made on
            // the device and is an identifier for one that came down from an account. Older
            // schemas have only Name.
            let joined = if exists(
                &s.db,
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('Shelf')
                 WHERE lower(name) = 'internalname')",
            )? {
                "sc.ShelfName IN (s.InternalName, s.Name)"
            } else {
                "sc.ShelfName = s.Name"
            };
            // Type is deliberately not filtered on: a real device is the only place to learn
            // which values it uses, and dropping rows on a guess would hide a collection rather
            // than show a spurious one.
            let mut query =
                s.db.prepare(&format!(
                    "SELECT DISTINCT s.Name, COALESCE(sc.ContentId, ''),
                     COALESCE(c.Title, ''), COALESCE(c.Attribution, '')
                     FROM Shelf s
                     LEFT JOIN ShelfContent sc
                       ON {joined}
                       AND COALESCE(sc._IsDeleted, 'false') NOT IN ('true', '1')
                     LEFT JOIN content c ON c.ContentID = sc.ContentId
                     WHERE COALESCE(s._IsDeleted, 'false') NOT IN ('true', '1')
                     ORDER BY s.Name COLLATE NOCASE,
                     COALESCE(NULLIF(c.Title, ''), sc.ContentId) COLLATE NOCASE,
                     COALESCE(sc.ContentId, '')"
                ))
                .map_err(schema)?;
            let rows = query
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        Shelved {
                            id: row.get(1)?,
                            title: row.get(2)?,
                            author: row.get(3)?,
                        },
                    ))
                })
                .map_err(schema)?;
            let mut shelves: Vec<Shelf> = Vec::new();
            for row in rows {
                let (name, book) = row.map_err(schema)?;
                if shelves.last().is_none_or(|shelf| shelf.name != name) {
                    shelves.push(Shelf {
                        name,
                        books: Vec::new(),
                    });
                }
                // An empty shelf joins to one row of nothing; it is still a shelf.
                if !book.id.is_empty() {
                    // A book on the shelf that is no longer on the device has no content row
                    // left. It is still on the shelf, so it is named by what remains of it.
                    let named = Shelved {
                        title: if book.title.is_empty() {
                            book.id.clone()
                        } else {
                            book.title.clone()
                        },
                        ..book
                    };
                    shelves.last_mut().expect("just pushed").books.push(named);
                }
            }
            Ok(shelves)
        })
    }

    fn encrypted_members(&self, volume: &str) -> Result<Vec<String>, KoboError> {
        self.with(|s| {
            let mut query =
                s.db.prepare("SELECT elementid FROM content_keys WHERE volumeid = ?1")
                    .map_err(schema)?;
            let names = query
                .query_map([volume], |r| r.get::<_, String>(0))
                .map_err(schema)?;
            names.collect::<rusqlite::Result<Vec<_>>>().map_err(schema)
        })
    }

    fn decrypt(&self, volume: &str, data: &[u8], serial: &str) -> Result<Vec<u8>, KoboError> {
        self.with(|s| {
            let serial = serial.trim();
            if serial.is_empty() {
                return Err(KoboError::new(
                    KoboErrorKind::Decrypt,
                    "Kobo serial must not be empty",
                ));
            }
            let mut query = s.db.prepare("SELECT UserID FROM user").map_err(schema)?;
            let users = query
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(schema)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(schema)?;
            if users.is_empty() {
                return Err(KoboError::new(
                    KoboErrorKind::Decrypt,
                    "Kobo database has no account IDs",
                ));
            }
            let mut keys = KeyStore::new();
            keys.kobo_keys = flamberge_keys::kobo::derive_userkeys(&[serial.to_string()], &users);
            keys.kobo_db = Some(s.bytes.clone());
            keys.kobo_volumeid = Some(volume.to_owned());
            flamberge_schemes::kobo::decrypt(data, &keys)
                .map(|result| result.data)
                .map_err(|e| {
                    KoboError::new(
                        KoboErrorKind::Decrypt,
                        format!(
                            "Kobo import failed; check the device serial and that this book is fully downloaded: {e}"
                        ),
                    )
                })
        })
    }
}
