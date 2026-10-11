//! Validated, verified book copies into a folder that only ever gains files.
use crate::{files, format, format::Format};
use ambit_caps_dir::{DirErrorKind, DirWrite, RelPath};
use anyhow::{ensure, Context, Result};

#[derive(Debug)]
pub struct Copied {
    /// Where it landed, inside the destination tree.
    pub path: RelPath,
    pub bytes: usize,
    pub already_present: bool,
}

/// Copies `data` as `name` into the existing folder `folder` of `destination`.
///
/// An identical file already there under the same name, compared ignoring case as FAT does, is
/// success; a different one is refused. The capability writes the file aside, flushes it, reads it
/// back and compares before giving it its name, and never replaces anything.
pub fn copy(
    data: &[u8],
    name: &str,
    destination: &dyn DirWrite,
    folder: &RelPath,
) -> Result<Copied> {
    let kind = Format::of(name);
    ensure!(
        kind.is_some()
            && !name.starts_with('.')
            && !name
                .chars()
                .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c)),
        "Choose an ordinary .epub, .pdf or .cbz filename suitable for the SD card"
    );
    format::validate(kind.unwrap_or_default(), data)
        .context("Book failed validation; nothing was copied")?;
    // Never create a missing folder here: an unplugged card must not silently turn a transfer
    // into a copy somewhere else.
    let meta = destination.metadata(folder).with_context(|| {
        "Destination must be an existing directory; mount the SD card and select a folder on it"
    })?;
    ensure!(
        meta.is_dir,
        "Destination must be an existing directory; mount the SD card and select a folder on it"
    );
    // Detect FAT-style case collisions even on a case-sensitive filesystem.
    if let Some(entry) = files::find_folded(destination, folder, name)? {
        let path = folder.join(&entry.name)?;
        ensure!(
            !entry.is_dir,
            "Destination name is already occupied; nothing was overwritten"
        );
        ensure!(
            files::book(destination, &path)? == data,
            "A different file already exists at {}; nothing was overwritten",
            files::slash(&path)
        );
        return Ok(Copied {
            path,
            bytes: data.len(),
            already_present: true,
        });
    }
    let free = destination
        .available_space(folder)
        .context("Cannot check destination free space")?;
    ensure!(
        free >= data.len() as u64,
        "Not enough free space on destination: need {} bytes, available {free}",
        data.len()
    );
    let path = folder.join(name)?;
    destination
        .create_new(&path, data)
        .map_err(|e| match e.kind {
            DirErrorKind::Exists => anyhow::anyhow!(
                "Cannot publish {}; existing files are never overwritten",
                files::slash(&path)
            ),
            DirErrorKind::Full => anyhow::anyhow!(
                "Destination filled up while copying; nothing was published: {}",
                e.message
            ),
            _ => anyhow::Error::new(e).context("Copy failed; nothing was published"),
        })?;
    Ok(Copied {
        path,
        bytes: data.len(),
        already_present: false,
    })
}
