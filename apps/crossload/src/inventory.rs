//! Read-only reader snapshots shared by sync and future TUI inventory views.
use crate::{crosspoint::Reader, epub, files, format::Format};
use ambit_caps_dir::{DirModify, RelPath};
use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    io::{Cursor, Read},
    sync::Arc,
};

#[derive(Clone, Debug)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub directory: bool,
}
#[derive(Clone, Debug)]
pub struct Identity {
    pub sha256: String,
    pub resources: Option<String>,
}
impl Identity {
    pub fn matches(&self, other: &Self) -> bool {
        self.sha256 == other.sha256
            || self
                .resources
                .as_ref()
                .is_some_and(|id| other.resources.as_ref() == Some(id))
    }
}
/// Where a book's metadata lives, as its own container names it.
fn package_document(zip: &mut zip::ZipArchive<Cursor<&[u8]>>) -> Option<String> {
    let mut text = String::new();
    zip.by_name("META-INF/container.xml")
        .ok()?
        .read_to_string(&mut text)
        .ok()?;
    let container = roxmltree::Document::parse_with_options(
        &text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .ok()?;
    container
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "rootfile")?
        .attribute("full-path")
        .map(str::to_owned)
}

/// Exact bytes, or identical ZIP resource names/content despite repackaging.
/// Title and ISBN alone never prove that two books are the same edition.
pub fn identity(bytes: &[u8]) -> Identity {
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    let resources = (|| -> Result<String> {
        epub::inspect(bytes)?;
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
        ensure!(
            zip.by_name("mimetype").is_ok() && zip.by_name("META-INF/container.xml").is_ok(),
            "Not EPUB"
        );
        // The package document carries what a book is called, not what it
        // says, so two copies differing only in their metadata are one book.
        // Without this, correcting an author or adding a series would split a
        // book from the copies of it already on a reader.
        let package = package_document(&mut zip);
        let mut names: Vec<String> = zip.file_names().map(str::to_owned).collect();
        names.sort();
        let mut digest = Sha256::new();
        for name in names {
            if name == "META-INF/xteink-device-profile.txt" {
                continue;
            }
            if package.as_deref() == Some(name.as_str()) {
                continue;
            }
            let mut file = zip.by_name(&name)?;
            if file.is_dir() {
                continue;
            }
            digest.update((name.len() as u64).to_le_bytes());
            digest.update(name.as_bytes());
            digest.update(file.size().to_le_bytes());
            let mut buffer = [0u8; 65536];
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                digest.update(&buffer[..read]);
            }
        }
        Ok(format!("{:x}", digest.finalize()))
    })()
    .ok();
    Identity { sha256, resources }
}
/// A mounted card: the one place books may be deleted from, so the one
/// folder that comes as a `DirModify`.
#[derive(Clone)]
pub struct Card {
    pub dir: Arc<dyn DirModify>,
    /// Where it is mounted, for messages.
    pub display: String,
}
// One per library, so the size of a reader is not worth a box.
#[allow(clippy::large_enum_variant)]
#[derive(Clone)]
pub enum Destination {
    Reader(Reader),
    Card(Card),
}
impl Destination {
    pub fn base(&self) -> &str {
        match self {
            Self::Reader(r) => r.folder(),
            Self::Card(_) => "/",
        }
    }
    pub fn files(&self) -> Result<Vec<FileEntry>> {
        match self {
            Self::Reader(r) => r.files(),
            Self::Card(card) => {
                let dir = &*card.dir;
                ensure!(
                    dir.metadata(&RelPath::root()).is_ok_and(|m| m.is_dir),
                    "Destination must be an existing mounted-card directory"
                );
                let mut pending = vec![(RelPath::root(), 0)];
                let mut found = Vec::new();
                while let Some((folder, depth)) = pending.pop() {
                    ensure!(depth <= 32, "Card directory tree is too deep");
                    for entry in dir.list(&folder)? {
                        let name = entry.name;
                        if name.starts_with('.')
                            || matches!(
                                name.to_lowercase().as_str(),
                                "xtcache" | "system volume information"
                            )
                        {
                            continue;
                        }
                        let path = folder.join(&name)?;
                        // The capability refuses links that lead off the card;
                        // what it lists is inside it.
                        let meta = dir.metadata(&path)?;
                        if entry.is_dir {
                            pending.push((path.clone(), depth + 1));
                        }
                        found.push(FileEntry {
                            path: files::slash(&path),
                            size: if entry.is_dir { 0 } else { meta.len },
                            directory: entry.is_dir,
                        });
                        ensure!(found.len() <= 20000, "Card inventory exceeds 20000 entries");
                    }
                }
                found.sort_by(|a, b| a.path.cmp(&b.path));
                Ok(found)
            }
        }
    }
    /// Delete one file. A reader reached over Wi-Fi cannot: this protocol
    /// uploads, downloads, renames and lists, and has no delete.
    pub fn remove(&self, file: &FileEntry) -> Result<()> {
        ensure!(!file.directory, "Refusing to remove a directory");
        match self {
            Self::Reader(_) => anyhow::bail!(
                "Deleting over Wi-Fi is not supported by the reader's protocol; mount its card and use --copy-to, or delete on the device"
            ),
            Self::Card(card) => {
                let path = files::rel(&file.path)?;
                ensure!(
                    !card.dir.metadata(&path)?.is_dir,
                    "Refusing to remove anything but a regular file"
                );
                card.dir.remove_file(&path)?;
                Ok(())
            }
        }
    }
    pub fn read(&self, file: &FileEntry) -> Result<Vec<u8>> {
        ensure!(
            !file.directory && file.size <= epub::MAX_BOOK_BYTES,
            "Cannot read oversized/directory inventory entry: {}",
            file.path
        );
        match self {
            Self::Reader(reader) => reader.read_file(&file.path, file.size),
            Self::Card(card) => {
                let bytes = files::read(&*card.dir, &files::rel(&file.path)?, file.size)
                    .with_context(|| {
                        format!("Card file changed during inventory: {}", file.path)
                    })?;
                ensure!(
                    bytes.len() as u64 == file.size,
                    "Card file changed during inventory: {}",
                    file.path
                );
                Ok(bytes)
            }
        }
    }
}
pub struct Existing {
    pub file: FileEntry,
    pub identity: Option<Identity>,
}
pub struct Inventory {
    pub entries: Vec<Existing>,
}
impl Inventory {
    pub fn scan(destination: &Destination, mut progress: impl FnMut(&str)) -> Result<Self> {
        let files = destination.files()?;
        let mut paths = HashSet::new();
        let mut entries = Vec::new();
        for file in files {
            ensure!(
                paths.insert(file.path.to_lowercase()),
                "Case-colliding entries in destination: {}",
                file.path
            );
            // Hashing a file the reader will never list means downloading it
            // over Wi-Fi for nothing.
            let identity =
                if !file.directory && Format::of(&file.path).is_some_and(Format::shown_on_reader) {
                    progress(&file.path);
                    Some(identity(&destination.read(&file)?))
                } else {
                    None
                };
            entries.push(Existing { file, identity });
        }
        Ok(Self { entries })
    }
    pub fn matching(&self, identities: &[&Identity]) -> Option<&Existing> {
        self.entries.iter().find(|entry| {
            entry
                .identity
                .as_ref()
                .is_some_and(|id| identities.iter().any(|other| id.matches(other)))
        })
    }
    pub fn at(&self, path: &str) -> Option<&Existing> {
        self.entries
            .iter()
            .find(|e| e.file.path.to_lowercase() == path.to_lowercase())
    }
}
