//! Reading and naming files through the directory capabilities.
use ambit_caps_dir::{Dir, DirErrorKind, RelPath};
use anyhow::{ensure, Context, Result};
use std::io::Read;

/// The whole of a regular file, refusing one larger than `limit`.
pub fn read(dir: &dyn Dir, path: &RelPath, limit: u64) -> Result<Vec<u8>> {
    let meta = dir
        .metadata(path)
        .with_context(|| format!("Cannot read {path}"))?;
    ensure!(!meta.is_dir, "Expected a regular file: {path}");
    ensure!(meta.len <= limit, "{path} exceeds {} MiB", limit >> 20);
    let mut data = Vec::new();
    dir.open(path)
        .with_context(|| format!("Cannot read {path}"))?
        .take(limit + 1)
        .read_to_end(&mut data)
        .with_context(|| format!("Cannot read {path}"))?;
    ensure!(
        data.len() as u64 <= limit,
        "{path} exceeds {} MiB",
        limit >> 20
    );
    Ok(data)
}

/// A book, up to the size any book may be.
pub fn book(dir: &dyn Dir, path: &RelPath) -> Result<Vec<u8>> {
    read(dir, path, crate::epub::MAX_BOOK_BYTES)
}

/// `"/a/b.epub"` or `"a/b.epub"`, as a path inside a tree. `"/"` is the tree itself.
pub fn rel(path: &str) -> Result<RelPath> {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return Ok(RelPath::root());
    }
    RelPath::parse(trimmed).with_context(|| format!("Invalid path {path}"))
}

/// A path inside a tree as the reader and the card name it: from the root, with a leading `/`.
pub fn slash(path: &RelPath) -> String {
    if path.is_root() {
        "/".into()
    } else {
        format!("/{path}")
    }
}

/// The end of the message for a device or card that is unplugged; a place whose scan fails with
/// it shows as "Not connected".
pub const NOT_CONNECTED: &str = "is not connected";

/// Fails with "`what` is not connected" when the tree isn't there right now (an unplugged Kobo
/// or card). Any other answer is for the caller's own checks.
pub fn connected(dir: &dyn Dir, what: &str) -> Result<()> {
    match dir.metadata(&RelPath::root()) {
        Err(e) if e.kind == DirErrorKind::Unavailable => {
            anyhow::bail!("{what} {NOT_CONNECTED}")
        }
        _ => Ok(()),
    }
}

/// Whether the error says the file or directory is not there.
pub fn missing(e: &anyhow::Error) -> bool {
    e.downcast_ref::<ambit_caps_dir::DirError>()
        .is_some_and(|e| e.kind == DirErrorKind::NotFound)
}

/// The entry in `parent` whose name equals `name` ignoring case, as FAT would see it.
pub fn find_folded(
    dir: &dyn Dir,
    parent: &RelPath,
    name: &str,
) -> Result<Option<ambit_caps_dir::Entry>> {
    let folded = name.to_lowercase();
    Ok(dir
        .list(parent)
        .with_context(|| format!("Cannot list {}", slash(parent)))?
        .into_iter()
        .find(|entry| entry.name.to_lowercase() == folded))
}
