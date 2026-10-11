//! ACSM requests: fulfilled through the `Adept` capability, published into the
//! import folder, and archived beside themselves.
use crate::{
    books::{Folder, Library},
    epub, files,
};
use ambit_caps_dir::{DirErrorKind, DirWrite, RelPath};
use anyhow::{ensure, Context, Result};
use crossload_adobe::Adept;
use sha2::{Digest, Sha256};

/// Fulfil the ACSM at `source` (a local path as the catalog names it) and
/// import its EPUB. Returns what to tell the user.
pub fn fulfil(library: &Library, source: &str, progress: &dyn Fn(&str)) -> Result<String> {
    let (root, path) = library.locate(source)?;
    let folder = library.folder(root);
    let data = files::read(&*folder.dir, &path, 8 << 20)?;
    let book = import(&*library.adept, &library.import, &data)?;
    let shown = format!("{}/{book}", library.import.display.trim_end_matches('/'));
    progress(&format!("Imported {shown}"));
    // The request is spent. Archiving it is a convenience, so a failure to
    // archive it must not call a completed fulfillment an error.
    let archived = match archive(&*folder.dir, &path, &data) {
        Ok(archived) => format!(
            " Archived the request as {}/{archived}.",
            folder.display.trim_end_matches('/')
        ),
        Err(e) => format!(" Book imported; the ACSM stays pending: {e:#}"),
    };
    Ok(format!("Imported {shown}.{archived}"))
}

/// Fulfil ACSM bytes and publish the EPUB into the import folder, never over
/// an existing file.
pub fn import(adept: &dyn Adept, output: &Folder, acsm: &[u8]) -> Result<RelPath> {
    // Check the destination before making any fulfillment requests.
    ensure!(
        output
            .dir
            .metadata(&RelPath::root())
            .is_ok_and(|m| m.is_dir),
        "The import folder {} is not available",
        output.display
    );
    let bytes = adept.fulfil(acsm).map_err(|e| anyhow::anyhow!(e.0))?;
    epub::inspect(&bytes).context("Downloaded file is not a supported EPUB")?;
    epub::font_metadata(&bytes).context("Unsupported encryption remains after decryption")?;
    epub::validate(&bytes)
        .context("Imported book failed EPUB validation; no output was published")?;
    let metadata = epub::metadata_from(&bytes)?.context("Missing EPUB metadata")?;
    let id = format!("{:x}", Sha256::digest(acsm));
    let target = RelPath::new([output_name(
        metadata.title.as_deref().unwrap_or("Book"),
        &id,
    )])?;
    output
        .dir
        .create_new(&target, &bytes)
        .map_err(|e| match e.kind {
            DirErrorKind::Exists => anyhow::anyhow!(
                "Cannot publish {target}; existing files are never overwritten. To transfer the existing local EPUB, use crossload send instead of importing again"
            ),
            _ => anyhow::Error::new(e).context(format!("Cannot publish {target}")),
        })?;
    Ok(target)
}

/// Keep a copy of a spent ACSM in an `archive/` folder beside it, so it stops
/// appearing as a pending request. The folder it is in only gains files, so
/// the request itself stays where it is: a request with an identical copy in
/// `archive/` is spent. A name already taken gains a suffix.
pub fn archive(dir: &dyn DirWrite, acsm: &RelPath, data: &[u8]) -> Result<RelPath> {
    let (parent, name) = acsm.split_last().context("ACSM has no file name")?;
    let directory = parent.join("archive")?;
    dir.create_dir(&directory)?;
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) => (stem.to_owned(), format!(".{extension}")),
        None => (name.to_owned(), String::new()),
    };
    for attempt in 0..1000 {
        let target = directory.join(&if attempt == 0 {
            format!("{stem}{extension}")
        } else {
            format!("{stem}-{attempt}{extension}")
        })?;
        match dir.create_new(&target, data) {
            Ok(()) => return Ok(target),
            Err(e) if e.kind == DirErrorKind::Exists => {
                // Already archived: the same request, twice.
                if files::read(dir, &target, 8 << 20).is_ok_and(|old| old == data) {
                    return Ok(target);
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("Too many archived copies of {stem}{extension}")
}

fn output_name(title: &str, id: &str) -> String {
    let mut title: String = title
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
    format!(
        "{} [{}].epub",
        if title.is_empty() { "Book" } else { title },
        &id[..12]
    )
}
