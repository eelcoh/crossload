//! Additive Kobo sync: inspect first, then explicitly apply missing books.
use crate::{
    books::Folder,
    files,
    inventory::{self, Destination, Identity, Inventory},
    kobo::{self, Device},
    prepare,
};
use ambit_caps_dir::RelPath;
use anyhow::{ensure, Context, Result};
use serde::Serialize;

pub struct Options {
    /// The import folder: where each Kobo book is kept locally.
    pub output: Folder,
    pub serial: Option<String>,
    pub apply: bool,
    /// The screen to make device copies for, or None to leave images alone.
    pub optimize: Option<crate::profile::Profile>,
    pub organized: bool,
    pub repair: bool,
    pub exclude: Vec<String>,
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
    pub excluded: usize,
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
    library: &Device,
    destination: &Destination,
    options: &Options,
    mut progress: impl FnMut(&str),
) -> Result<Report> {
    ensure!(
        !options.repair || matches!(destination, Destination::Reader(_)),
        "--repair is supported for Wi-Fi sync only"
    );
    let books = library.books()?;
    for id in &options.exclude {
        ensure!(
            books.iter().any(|b| &b.id == id),
            "Unknown excluded Kobo book ID: {id}"
        );
    }
    progress("Reading destination inventory (this can take time over Wi-Fi)…");
    let inventory = Inventory::scan(destination, |path| progress(&format!("Checking {path}")))?;
    let mut report = Report {
        apply: options.apply,
        previews_skipped: books.iter().filter(|b| b.preview).count(),
        excluded: books
            .iter()
            .filter(|b| !b.preview && options.exclude.contains(&b.id))
            .count(),
        books: vec![],
    };
    // This records only planned or successfully transferred copies, never failed jobs.
    let mut planned: Vec<(Identity, Identity, String)> = Vec::new();
    for book in books
        .into_iter()
        .filter(|b| !b.preview && !options.exclude.contains(&b.id))
    {
        progress(&format!(
            "{}: {}",
            if options.apply { "Syncing" } else { "Planning" },
            book.title
        ));
        let result = (|| -> Result<(Status, String, String)> {
            let (original, original_bytes) = library.import(&book.id, options.serial.as_deref())?;
            let original_id = inventory::identity(&original_bytes);
            let prepared = prepare::prepare(
                original_bytes.clone(),
                &original,
                options.optimize,
                options.organized,
            )?;
            let data = &prepared.data;
            let prepared_id = inventory::identity(data);
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
            let name = &prepared.name;
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
                let conflict = format!("Different contents occupy {target}; nothing was overwritten. To keep the reader's copy, rerun with --exclude {}", book.id);
                ensure!(
                    matches!(destination, Destination::Reader(_))
                        && !existing.file.directory
                        && existing.file.size < data.len() as u64,
                    "{conflict}"
                );
                let partial = destination.read(&existing.file)?;
                ensure!(data.starts_with(&partial), "{conflict}");
                ensure!(options.repair, "Verified incomplete upload at {target}; use --apply --repair to preserve it and send a fresh copy");
                true
            } else {
                false
            };
            let mut actual_target = target.clone();
            let mut detail = format!("{} → {} bytes", original_bytes.len(), data.len());
            if options.apply {
                let local = kobo::publish(&*options.output.dir, &original, &original_bytes, true)?;
                ensure!(
                    inventory::identity(&files::book(&*options.output.dir, &local)?).sha256
                        == original_id.sha256,
                    "Kobo book changed during sync; local import retained, rerun sync"
                );
                let local = format!("{}/{local}", options.output.display.trim_end_matches('/'));
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
                                .send(&prepared.name, &prepared.data)?;
                            actual_target = sent.path;
                            if let Some(backup) = sent.backup {
                                detail
                                    .push_str(&format!("; incomplete file preserved at {backup}"));
                            }
                        }
                        Destination::Card(card) => {
                            let directory = if options.organized {
                                prepare::author_directory(
                                    &*card.dir,
                                    &RelPath::root(),
                                    &prepared.author,
                                )?
                            } else {
                                RelPath::root()
                            };
                            let copied = crate::copy::copy(
                                &prepared.data,
                                &prepared.name,
                                &*card.dir,
                                &directory,
                            )?;
                            actual_target = files::slash(&copied.path);
                        }
                    }
                    Ok(())
                })();
                transfer.with_context(|| {
                    format!("Local import retained at {local}; rerun sync to recover")
                })?;
                detail.push_str(&format!("; local: {local}"));
            } else {
                // Detect an obvious recovery conflict without writing anything.
                let local = RelPath::new([original.as_str()])?;
                match options.output.dir.metadata(&local) {
                    Ok(meta) => ensure!(
                        !meta.is_dir
                            && meta.len == original_bytes.len() as u64
                            && files::book(&*options.output.dir, &local)? == original_bytes,
                        "Existing local import differs: {}/{local}",
                        options.output.display.trim_end_matches('/')
                    ),
                    Err(e) if e.kind == ambit_caps_dir::DirErrorKind::NotFound => {}
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
