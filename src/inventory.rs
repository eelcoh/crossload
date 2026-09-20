//! Read-only reader snapshots shared by sync and future TUI inventory views.
use crate::{crosspoint::Reader, epub, format::Format};
use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
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
        let mut names: Vec<String> = zip.file_names().map(str::to_owned).collect();
        names.sort();
        let mut digest = Sha256::new();
        for name in names {
            if name == "META-INF/xteink-device-profile.txt" {
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
#[derive(Clone)]
pub enum Destination {
    Reader(Reader),
    Card(PathBuf),
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
            Self::Card(root) => {
                ensure!(
                    root.is_dir(),
                    "Destination must be an existing mounted-card directory"
                );
                let mut pending = vec![(root.clone(), String::new(), 0)];
                let mut files = Vec::new();
                while let Some((folder, prefix, depth)) = pending.pop() {
                    ensure!(depth <= 32, "Card directory tree is too deep");
                    for entry in fs::read_dir(folder)? {
                        let entry = entry?;
                        let name = entry
                            .file_name()
                            .into_string()
                            .map_err(|_| anyhow::anyhow!("Card filename is not UTF-8"))?;
                        if name.starts_with('.')
                            || matches!(
                                name.to_lowercase().as_str(),
                                "xtcache" | "system volume information"
                            )
                        {
                            continue;
                        }
                        let kind = entry.file_type()?;
                        ensure!(
                            !kind.is_symlink(),
                            "Inventory refuses symlink: {}",
                            entry.path().display()
                        );
                        ensure!(
                            kind.is_file() || kind.is_dir(),
                            "Inventory refuses special filesystem entries"
                        );
                        let path = format!("{prefix}/{name}");
                        if kind.is_dir() {
                            pending.push((entry.path(), path.clone(), depth + 1));
                        }
                        files.push(FileEntry {
                            path,
                            size: entry.metadata()?.len(),
                            directory: kind.is_dir(),
                        });
                        ensure!(files.len() <= 20000, "Card inventory exceeds 20000 entries");
                    }
                }
                files.sort_by(|a, b| a.path.cmp(&b.path));
                Ok(files)
            }
        }
    }
    /// Resolve a card entry to a real path, refusing anything that escapes the
    /// card or turned into a symlink since it was listed.
    fn on_card(root: &Path, path: &str) -> Result<PathBuf> {
        let relative = Path::new(path.strip_prefix('/').context("Invalid card path")?);
        ensure!(
            relative
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
            "Invalid card path"
        );
        let mut resolved = root.to_path_buf();
        for component in relative.components() {
            resolved.push(component);
            ensure!(
                !fs::symlink_metadata(&resolved)?.file_type().is_symlink(),
                "Card entry became a symlink"
            );
        }
        Ok(resolved)
    }
    /// Delete one file. A reader reached over Wi-Fi cannot: this protocol
    /// uploads, downloads, renames and lists, and has no delete.
    pub fn remove(&self, file: &FileEntry) -> Result<()> {
        ensure!(!file.directory, "Refusing to remove a directory");
        match self {
            Self::Reader(_) => anyhow::bail!(
                "Deleting over Wi-Fi is not supported by the reader's protocol; mount its card and use --copy-to, or delete on the device"
            ),
            Self::Card(root) => {
                let path = Self::on_card(root, &file.path)?;
                ensure!(
                    fs::symlink_metadata(&path)?.is_file(),
                    "Refusing to remove anything but a regular file"
                );
                fs::remove_file(path)?;
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
            Self::Card(root) => {
                let path = Self::on_card(root, &file.path)?;
                let file_on_disk = fs::File::open(path)?;
                ensure!(
                    file_on_disk.metadata()?.is_file(),
                    "Card entry is no longer a file"
                );
                let mut bytes = Vec::new();
                file_on_disk.take(file.size + 1).read_to_end(&mut bytes)?;
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
            let identity = if !file.directory && Format::of(&file.path).is_some() {
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
