//! Additive Kobo sync: inspect first, then explicitly apply missing books.
use crate::{
    inventory::{self, Destination, Identity, Inventory},
    kobo::Library,
    prepare,
};
use anyhow::{ensure, Context, Result};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub struct Options {
    pub output: PathBuf,
    pub serial: Option<String>,
    pub apply: bool,
    pub optimize: bool,
    pub organized: bool,
    pub repair: bool,
}
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Present,
    Duplicate,
    Transfer,
    Repair,
    Sent,
    Repaired,
    Failed,
}
#[derive(Debug, Serialize)]
pub struct Row {
    pub id: String,
    pub title: String,
    pub status: Status,
    pub path: Option<String>,
    pub detail: String,
}
#[derive(Debug, Serialize)]
pub struct Report {
    pub apply: bool,
    pub previews_skipped: usize,
    pub books: Vec<Row>,
}
impl Report {
    pub fn failures(&self) -> usize {
        self.books
            .iter()
            .filter(|r| r.status == Status::Failed)
            .count()
    }
}

/// Dry runs may decrypt/optimize in temporary storage, but never publish files or
/// write the reader. Failed inventory means unknown state and aborts the run.
pub fn run(
    library: &Library,
    destination: &Destination,
    options: &Options,
    mut progress: impl FnMut(&str),
) -> Result<Report> {
    ensure!(
        !options.repair || matches!(destination, Destination::Reader(_)),
        "--repair is supported for Wi-Fi sync only"
    );
    let books = library.books()?;
    progress("Reading destination inventory (this can take time over Wi-Fi)…");
    let inventory = Inventory::scan(destination, |path| progress(&format!("Checking {path}")))?;
    let mut report = Report {
        apply: options.apply,
        previews_skipped: books.iter().filter(|b| b.preview).count(),
        books: vec![],
    };
    // This records only planned or successfully transferred copies, never failed jobs.
    let mut planned: Vec<(Identity, Identity, String)> = Vec::new();
    for book in books.into_iter().filter(|b| !b.preview) {
        progress(&format!(
            "{}: {}",
            if options.apply { "Syncing" } else { "Planning" },
            book.title
        ));
        let result = (|| -> Result<(Status, String, String)> {
            let staging = tempfile::tempdir()?;
            let original = library.import(&book.id, staging.path(), options.serial.as_deref())?;
            let original_bytes = fs::read(&original)?;
            let original_id = inventory::identity(&original_bytes);
            let prepared = prepare::prepare(&original, options.optimize, options.organized)?;
            let data = fs::read(&prepared.path)?;
            let prepared_id = inventory::identity(&data);
            if let Some(existing) = inventory.matching(&[&original_id, &prepared_id]) {
                // Recheck before declaring success during apply; names/sizes alone
                // cannot prove a file has remained unchanged since the initial scan.
                if options.apply {
                    let current = inventory::identity(&destination.read(&existing.file)?);
                    ensure!(
                        current.matches(&original_id) || current.matches(&prepared_id),
                        "Destination changed since inventory; rerun sync"
                    );
                }
                return Ok((
                    Status::Present,
                    existing.file.path.clone(),
                    "Verified matching book contents".into(),
                ));
            }
            if let Some((_, _, path)) = planned.iter().find(|(a, b, _)| {
                a.matches(&original_id)
                    || a.matches(&prepared_id)
                    || b.matches(&original_id)
                    || b.matches(&prepared_id)
            }) {
                return Ok((
                    Status::Duplicate,
                    path.clone(),
                    "Same content as an earlier book in this run".into(),
                ));
            }
            let folder = if options.organized {
                format!(
                    "{}/{}",
                    destination.base().trim_end_matches('/'),
                    prepared.author
                )
            } else {
                destination.base().to_owned()
            };
            let name = prepared
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .context("Invalid EPUB name")?;
            let target = format!("{}/{name}", folder.trim_end_matches('/'));
            if let Some(parent) = inventory.at(&folder) {
                ensure!(
                    parent.file.directory,
                    "Author folder is occupied by a file: {folder}"
                );
            }
            ensure!(
                !planned
                    .iter()
                    .any(|(_, _, p)| p.to_lowercase() == target.to_lowercase()),
                "Different books map to the same destination: {target}"
            );
            let repair = if let Some(existing) = inventory.at(&target) {
                ensure!(options.repair && !existing.file.directory && existing.file.size < data.len() as u64,
                    "A different file exists at {target}; nothing will be overwritten. For an incomplete Wi-Fi upload use --apply --repair");
                let partial = destination.read(&existing.file)?;
                ensure!(
                    data.starts_with(&partial),
                    "Existing file is not a matching incomplete prefix: {target}"
                );
                true
            } else {
                false
            };
            let mut actual_target = target.clone();
            let mut detail = format!("{} → {} bytes", original_bytes.len(), data.len());
            if options.apply {
                let local =
                    library.import_reusing(&book.id, &options.output, options.serial.as_deref())?;
                ensure!(
                    inventory::identity(&fs::read(&local)?).sha256 == original_id.sha256,
                    "Kobo book changed during sync; local import retained, rerun sync"
                );
                let transfer = (|| -> Result<()> {
                    match destination {
                        Destination::Reader(reader) => {
                            let reader = if options.organized {
                                reader.for_author(&prepared.author)?
                            } else {
                                reader.clone()
                            };
                            let sent = reader
                                .repair_incomplete(options.repair)
                                .send(&prepared.path)?;
                            actual_target = sent.path;
                            if let Some(backup) = sent.backup {
                                detail
                                    .push_str(&format!("; incomplete file preserved at {backup}"));
                            }
                        }
                        Destination::Card(root) => {
                            let directory = if options.organized {
                                prepare::author_directory(root, &prepared.author)?
                            } else {
                                root.clone()
                            };
                            let copied = crate::copy::copy(&prepared.path, &directory)?;
                            actual_target =
                                format!("/{}", copied.path.strip_prefix(root)?.to_string_lossy());
                        }
                    }
                    Ok(())
                })();
                transfer.with_context(|| {
                    format!(
                        "Local import retained at {}; rerun sync to recover",
                        local.display()
                    )
                })?;
                detail.push_str(&format!("; local: {}", local.display()));
            } else {
                // Detect an obvious recovery conflict without creating the output directory.
                let local = options
                    .output
                    .join(original.file_name().context("Invalid import filename")?);
                match fs::symlink_metadata(&local) {
                    Ok(meta) => ensure!(
                        meta.is_file()
                            && meta.len() == original_bytes.len() as u64
                            && fs::read(&local)? == original_bytes,
                        "Existing local import differs: {}",
                        local.display()
                    ),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
            }
            planned.push((original_id, prepared_id, actual_target.clone()));
            Ok((
                match (options.apply, repair) {
                    (false, false) => Status::Transfer,
                    (false, true) => Status::Repair,
                    (true, false) => Status::Sent,
                    (true, true) => Status::Repaired,
                },
                actual_target,
                detail,
            ))
        })();
        let (status, path, detail) = match result {
            Ok((s, p, d)) => (s, Some(p), d),
            Err(e) => (Status::Failed, None, format!("{e:#}")),
        };
        report.books.push(Row {
            id: book.id,
            title: book.title,
            status,
            path,
            detail,
        });
    }
    Ok(report)
}

pub fn card(path: &Path) -> Result<Destination> {
    ensure!(path.is_dir(), "Select an existing mounted-card directory");
    Ok(Destination::Card(path.canonicalize()?))
}
