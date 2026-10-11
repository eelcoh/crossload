//! What only a Kobo's own database knows: which store books it holds, the collections and
//! highlights made on it, and the keys that open its store books.
//!
//! Read-only. The database is the device's: an implementation reads a snapshot of it and never
//! writes back. The book files themselves are not here: the app reads them through the `Dir` it
//! gets for the Kobo's mount.
#![forbid(unsafe_code)]

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;

/// A row of the device's `content` table that names a book: a store book, or the entry the Kobo
/// made for a sideloaded file. Whether its file is on the device is for the app to check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreBook {
    /// `ContentID`: for a store book, also its file's name in `.kobo/kepub`.
    pub id: String,
    pub title: String,
    pub author: String,
    /// The database holds keys for it: the file is encrypted.
    pub encrypted: bool,
    /// A sample, not the full edition (`Accessibility = 6`).
    pub preview: bool,
}

/// Something the reader marked while reading.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Annotation {
    pub title: String,
    pub author: String,
    /// The passage highlighted, which a note on its own leaves empty.
    pub text: String,
    /// What was written about it, if anything.
    pub note: String,
    pub created: String,
    /// How far into the chapter it sits, from 0 to 1.
    pub progress: f64,
}

impl Annotation {
    /// Which book this belongs to, for gathering a reader's marks together.
    pub fn book(&self) -> (&str, &str) {
        (&self.title, &self.author)
    }
}

/// A collection made on the device, with the books on it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Shelf {
    pub name: String,
    pub books: Vec<Shelved>,
}

/// One book on a shelf, as the shelf records it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Shelved {
    /// The same ID `crossload kobo list` prints, so a shelf can be acted on.
    pub id: String,
    pub title: String,
    pub author: String,
}

/// Read access to one Kobo's database. Blocking: call it from a task.
pub trait Kobo: Send + Sync + 'static {
    /// Every book the database lists, previews included, in no particular order.
    fn books(&self) -> Result<Vec<StoreBook>, KoboError>;
    /// The collections, sorted by name, each with its books.
    fn shelves(&self) -> Result<Vec<Shelf>, KoboError>;
    /// Highlights and notes (not bare bookmarks), by book and then position.
    fn annotations(&self) -> Result<Vec<Annotation>, KoboError>;
    /// The archive members the database holds keys for in this volume; empty when it holds
    /// none, which means the volume is not encrypted.
    fn encrypted_members(&self, volume: &str) -> Result<Vec<String>, KoboError>;
    /// Decrypts `data`, the file of store volume `volume`, with keys derived from the device
    /// serial and the accounts in the database. Returns the archive as it was before
    /// encryption.
    fn decrypt(&self, volume: &str, data: &[u8], serial: &str) -> Result<Vec<u8>, KoboError>;
}

impl<T: Kobo + ?Sized> Kobo for Arc<T> {
    fn books(&self) -> Result<Vec<StoreBook>, KoboError> {
        (**self).books()
    }
    fn shelves(&self) -> Result<Vec<Shelf>, KoboError> {
        (**self).shelves()
    }
    fn annotations(&self) -> Result<Vec<Annotation>, KoboError> {
        (**self).annotations()
    }
    fn encrypted_members(&self, volume: &str) -> Result<Vec<String>, KoboError> {
        (**self).encrypted_members(volume)
    }
    fn decrypt(&self, volume: &str, data: &[u8], serial: &str) -> Result<Vec<u8>, KoboError> {
        (**self).decrypt(volume, data, serial)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KoboErrorKind {
    /// No database where one was expected: not a Kobo, or not mounted.
    NotFound,
    /// The database changed while it was being read.
    Changed,
    /// A schema this does not understand.
    Unsupported,
    /// The keys did not open the book.
    Decrypt,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KoboError {
    pub kind: KoboErrorKind,
    pub message: String,
}

impl KoboError {
    pub fn new(kind: KoboErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for KoboError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for KoboError {}

/// A Kobo database for tests, filled in by the test. Clones share the same contents.
#[derive(Clone, Default)]
pub struct SimKobo {
    state: Arc<Mutex<SimState>>,
}

#[derive(Default)]
struct SimState {
    missing: bool,
    books: Vec<StoreBook>,
    shelves: Vec<Shelf>,
    annotations: Vec<Annotation>,
    /// (volume, members, serial, plain text)
    keys: Vec<(String, Vec<String>, String, Vec<u8>)>,
    decrypted: Vec<String>,
}

impl SimKobo {
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> MutexGuard<'_, SimState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// No database: every call fails with `NotFound`, as for an unmounted Kobo.
    pub fn missing(&self) -> &Self {
        self.state().missing = true;
        self
    }

    pub fn book(&self, book: StoreBook) -> &Self {
        self.state().books.push(book);
        self
    }

    pub fn shelf(&self, shelf: Shelf) -> &Self {
        self.state().shelves.push(shelf);
        self
    }

    pub fn annotation(&self, annotation: Annotation) -> &Self {
        self.state().annotations.push(annotation);
        self
    }

    /// Volume `volume` is encrypted (its keys cover `members`); decrypting it with `serial`
    /// gives `plain`, whatever the encrypted bytes are.
    pub fn keys(&self, volume: &str, members: &[&str], serial: &str, plain: &[u8]) -> &Self {
        self.state().keys.push((
            volume.to_string(),
            members.iter().map(|m| m.to_string()).collect(),
            serial.to_string(),
            plain.to_vec(),
        ));
        self
    }

    /// The volumes decrypted so far, in order.
    pub fn decrypted(&self) -> Vec<String> {
        self.state().decrypted.clone()
    }

    fn check(&self) -> Result<MutexGuard<'_, SimState>, KoboError> {
        let state = self.state();
        if state.missing {
            return Err(KoboError::new(
                KoboErrorKind::NotFound,
                "No .kobo/KoboReader.sqlite found",
            ));
        }
        Ok(state)
    }
}

impl Kobo for SimKobo {
    fn books(&self) -> Result<Vec<StoreBook>, KoboError> {
        Ok(self.check()?.books.clone())
    }
    fn shelves(&self) -> Result<Vec<Shelf>, KoboError> {
        Ok(self.check()?.shelves.clone())
    }
    fn annotations(&self) -> Result<Vec<Annotation>, KoboError> {
        Ok(self.check()?.annotations.clone())
    }
    fn encrypted_members(&self, volume: &str) -> Result<Vec<String>, KoboError> {
        Ok(self
            .check()?
            .keys
            .iter()
            .find(|(v, ..)| v == volume)
            .map(|(_, members, ..)| members.clone())
            .unwrap_or_default())
    }
    fn decrypt(&self, volume: &str, _data: &[u8], serial: &str) -> Result<Vec<u8>, KoboError> {
        let mut state = self.check()?;
        let plain = state
            .keys
            .iter()
            .find(|(v, _, s, _)| v == volume && s == serial)
            .map(|(.., plain)| plain.clone())
            .ok_or_else(|| {
                KoboError::new(KoboErrorKind::Decrypt, "Kobo keys did not open this book")
            })?;
        state.decrypted.push(volume.to_string());
        Ok(plain)
    }
}
