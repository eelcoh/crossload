//! Validated, atomic EPUB copies to an existing mounted-card directory.
use crate::epub;
use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub struct Copied {
    pub path: PathBuf,
    pub bytes: usize,
    pub already_present: bool,
}

pub fn copy(book: &Path, destination: &Path) -> Result<Copied> {
    // Never create a missing mount point: an unplugged card must not silently
    // turn a transfer into a copy somewhere on the computer's own disk.
    ensure!(
        destination.is_dir(),
        "Destination must be an existing directory; mount the SD card and select a folder on it"
    );
    let destination = destination.canonicalize()?;
    let name = book
        .file_name()
        .and_then(|n| n.to_str())
        .context("EPUB filename must be UTF-8")?;
    ensure!(
        name.to_ascii_lowercase().ends_with(".epub")
            && !name.starts_with('.')
            && !name
                .chars()
                .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c)),
        "Choose an ordinary .epub filename suitable for the SD card"
    );
    let data = read(book)?;
    epub::validate(&data).context("Book failed EPUB validation; nothing was copied")?;
    epub::font_metadata(&data)
        .context("Book still has unsupported encryption; nothing was copied")?;
    let target = destination.join(name);
    // Detect FAT-style case collisions even when tests run on a case-sensitive
    // filesystem. Do not follow an existing destination symlink.
    for entry in fs::read_dir(&destination)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().to_lowercase() == name.to_lowercase() {
            ensure!(
                entry.file_type()?.is_file(),
                "Destination name is already occupied; nothing was overwritten"
            );
            ensure!(
                read(&entry.path())? == data,
                "A different file already exists at {}; nothing was overwritten",
                entry.path().display()
            );
            return Ok(Copied {
                path: entry.path(),
                bytes: data.len(),
                already_present: true,
            });
        }
    }
    let free = fs2::available_space(&destination).context("Cannot check destination free space")?;
    ensure!(
        free >= data.len() as u64,
        "Not enough free space on destination: need {} bytes, available {free}",
        data.len()
    );
    let mut stage = tempfile::Builder::new()
        .prefix(".xteink-")
        .suffix(".uploading")
        .tempfile_in(&destination)?;
    stage
        .write_all(&data)
        .context("SD-card write failed; no EPUB was published")?;
    stage
        .as_file()
        .sync_all()
        .context("Could not flush SD-card data; no EPUB was published")?;
    let written = read(stage.path())?;
    ensure!(
        written.len() == data.len() && Sha256::digest(&written) == Sha256::digest(&data),
        "Copied bytes failed verification; no EPUB was published"
    );
    stage.persist_noclobber(&target).with_context(|| {
        format!(
            "Cannot publish {}; existing files are never overwritten",
            target.display()
        )
    })?;
    Ok(Copied {
        path: target,
        bytes: data.len(),
        already_present: false,
    })
}

fn read(path: &Path) -> Result<Vec<u8>> {
    let file = File::open(path).with_context(|| format!("Cannot read {}", path.display()))?;
    ensure!(file.metadata()?.is_file(), "Expected a regular EPUB file");
    let mut data = Vec::new();
    file.take(epub::MAX_BOOK_BYTES + 1).read_to_end(&mut data)?;
    ensure!(
        data.len() as u64 <= epub::MAX_BOOK_BYTES,
        "EPUB exceeds 128 MiB"
    );
    Ok(data)
}
