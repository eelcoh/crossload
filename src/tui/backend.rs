use super::{Entry, Options, Source};
use crate::{adobe, copy, crosspoint, kobo, prepare};
use anyhow::{Context, Result};
use std::fs;
pub(super) fn load(options: &Options, local: bool) -> Result<Vec<Entry>> {
    if !local {
        return kobo::Library::open(
            options
                .device
                .as_ref()
                .context("No Kobo selected; use --device")?,
        )?
        .books()?
        .into_iter()
        .filter(|b| options.show_previews || !b.preview)
        .map(|b| {
            Ok(Entry {
                title: b.title,
                author: b.author,
                kind: if b.preview { "Preview" } else { "Kobo" },
                source: Source::Kobo(b.id),
                preview: b.preview,
            })
        })
        .collect();
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(&options.browse).context("Cannot browse directory")? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            entries.push(Entry {
                title: name,
                author: String::new(),
                kind: "Folder",
                source: Source::Directory(path),
                preview: false,
            });
        } else if kind.is_file() {
            let extension = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !matches!(extension.as_str(), "epub" | "acsm" | "ascm") {
                continue;
            }
            entries.push(Entry {
                title: name,
                author: String::new(),
                kind: if extension == "epub" { "EPUB" } else { "ACSM" },
                source: Source::Local(path),
                preview: false,
            });
        }
    }
    entries.sort_by_key(|e| {
        (
            !matches!(e.source, Source::Directory(_)),
            e.title.to_lowercase(),
        )
    });
    Ok(entries)
}
pub(super) fn perform(options: Options, entry: Entry, progress: impl Fn(&str)) -> Result<String> {
    let path = match entry.source {
        Source::Kobo(id) => {
            progress("Importing Kobo book…");
            kobo::Library::open(options.device.as_ref().context("No Kobo device")?)?.import(
                &id,
                &options.output,
                options.serial.as_deref(),
            )?
        }
        Source::Local(path) if entry.kind == "ACSM" => {
            progress("Fulfilling ACSM and importing EPUB…");
            let state = options
                .state
                .clone()
                .map(Ok)
                .unwrap_or_else(adobe::default_state_dir)?;
            adobe::Store::open(&state)?.import(&path, &options.output)?
        }
        Source::Local(path) => {
            if options.send_to.is_none() && options.copy_to.is_none() {
                progress("Importing local EPUB…");
                fs::create_dir_all(&options.output)?;
                copy::copy(&path, &options.output)?.path
            } else {
                path
            }
        }
        Source::Directory(_) => anyhow::bail!("Select a book"),
    };
    let local = format!("Local EPUB: {}", path.display());
    if options.send_to.is_none() && options.copy_to.is_none() {
        return Ok(format!("{local}. Use --send-to or --copy-to to transfer."));
    }
    let transfer = || -> Result<String> {
        progress("Preparing device copy…");
        let prepared = prepare::prepare(&path, options.optimize, options.organized)?;
        if let Some(address) = &options.send_to {
            progress("Sending to CrossPoint and verifying contents…");
            let reader = crosspoint::Reader::new(address, &options.folder)?;
            let reader = if options.organized {
                reader.for_author(&prepared.author)?
            } else {
                reader
            };
            let sent = reader.send(&prepared.path)?;
            Ok(format!(
                "{} {} ({} bytes; verified). {local}",
                if sent.already_present {
                    "Already on reader:"
                } else {
                    "Sent"
                },
                sent.path,
                sent.bytes
            ))
        } else {
            progress("Copying to card and verifying contents…");
            let destination = options
                .copy_to
                .as_ref()
                .context("Missing card destination")?;
            let destination = if options.organized {
                prepare::author_directory(destination, &prepared.author)?
            } else {
                destination.clone()
            };
            let copied = copy::copy(&prepared.path, &destination)?;
            Ok(format!(
                "{} {} ({} bytes; verified). Safely eject the card. {local}",
                if copied.already_present {
                    "Already on card:"
                } else {
                    "Copied"
                },
                copied.path.display(),
                copied.bytes
            ))
        }
    };
    transfer().with_context(|| {
        format!("{local} remains intact; retry from Local or use crossload send/copy")
    })
}

/// Read-only snapshot and the same exact/resource identities used by sync.
pub(super) fn presence(options: &Options, entries: Vec<Entry>) -> Result<Vec<(Source, String)>> {
    use crate::inventory::{self, Destination, Inventory};
    let destination = if let Some(address) = &options.send_to {
        Destination::Reader(crosspoint::Reader::new(address, &options.folder)?)
    } else {
        crate::sync::card(
            options
                .copy_to
                .as_ref()
                .context("No destination selected")?,
        )?
    };
    let inventory = Inventory::scan(&destination, |_| {})?;
    let library = if entries.iter().any(|e| matches!(e.source, Source::Kobo(_))) {
        options
            .device
            .as_ref()
            .map(|p| kobo::Library::open(p))
            .transpose()?
    } else {
        None
    };
    let mut results = Vec::new();
    for entry in entries {
        if entry.preview || matches!(entry.source, Source::Directory(_)) || entry.kind == "ACSM" {
            continue;
        }
        let check = (|| -> Result<bool> {
            let staging = tempfile::tempdir()?;
            let path = match &entry.source {
                Source::Kobo(id) => library.as_ref().context("No Kobo")?.import(
                    id,
                    staging.path(),
                    options.serial.as_deref(),
                )?,
                Source::Local(path) => path.clone(),
                Source::Directory(_) => unreachable!(),
            };
            let prepared = prepare::prepare(&path, options.optimize, options.organized)?;
            let original = inventory::identity(&fs::read(&path)?);
            let optimized = inventory::identity(&fs::read(&prepared.path)?);
            Ok(inventory.matching(&[&original, &optimized]).is_some())
        })();
        results.push((
            entry.source,
            match check {
                Ok(true) => "Present".into(),
                Ok(false) => "Missing".into(),
                Err(_) => "Unknown".into(),
            },
        ));
    }
    Ok(results)
}
