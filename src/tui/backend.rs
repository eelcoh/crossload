use super::{Entry, Options, Source};
use crate::{adobe, copy, crosspoint, prepare};
use anyhow::{Context, Result};
use std::fs;
pub(super) fn library_options(options: &Options) -> crate::books::Options {
    crate::books::Options {
        show_previews: options.show_previews,
        local: options.browse.clone(),
        output: options.output.clone(),
        kobo: options.device.clone(),
        reader: options.send_to.clone(),
        card: options.copy_to.clone(),
        folder: options.folder.clone(),
        serial: options.serial.clone(),
        optimize: options.optimize,
        organized: options.organized,
        cache: None,
    }
}
/// Copy every chosen book, reporting each one and stopping cleanly when asked.
/// One failure does not abandon the rest; the summary names what went wrong.
pub(super) fn perform(
    options: Options,
    entries: Vec<Entry>,
    target: crate::books::Place,
    cancel: &std::sync::atomic::AtomicBool,
    progress: impl Fn(&str),
) -> Result<String> {
    let total = entries.len();
    let mut done = 0;
    let mut failures: Vec<String> = vec![];
    let mut last = String::new();
    let mut stopped = false;
    for (index, entry) in entries.into_iter().enumerate() {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            stopped = true;
            break;
        }
        let title = entry.title.clone();
        if total > 1 {
            progress(&format!("Copying {} of {total}: {title}…", index + 1));
        }
        match one(&options, entry, target, &progress) {
            Ok(message) => {
                done += 1;
                last = message;
            }
            Err(e) => failures.push(format!("{title}: {e:#}")),
        }
    }
    if total == 1 && failures.is_empty() {
        return Ok(format!("{last} Press r to refresh."));
    }
    if total == 1 {
        anyhow::bail!("{}", failures.remove(0));
    }
    let mut summary = format!(
        "Copied {done} of {} to {}.",
        crate::books::books(total),
        target.label()
    );
    if stopped {
        summary.push_str(" Stopped on request.");
    }
    if !failures.is_empty() {
        summary.push_str(&format!(
            " {} failed: {}{}",
            failures.len(),
            failures
                .iter()
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
                .join("; "),
            if failures.len() > 2 { "; …" } else { "" }
        ));
    }
    summary.push_str(" Press r to refresh.");
    Ok(summary)
}
fn one(
    options: &Options,
    entry: Entry,
    target: crate::books::Place,
    progress: &impl Fn(&str),
) -> Result<String> {
    let path = match entry.source {
        Source::Book(book) => {
            return crate::books::transfer(&library_options(options), &book, target, progress)
        }
        Source::Local(path) if entry.kind == "ACSM" => {
            progress("Fulfilling ACSM and importing EPUB…");
            let state = options
                .state
                .clone()
                .map(Ok)
                .unwrap_or_else(adobe::default_state_dir)?;
            let book = adobe::Store::open(&state)?.import(&path, &options.output)?;
            // The request is spent. Archiving it is a convenience, so a failure
            // to move it must not call a completed fulfillment an error.
            match adobe::Store::archive(&path) {
                Ok(archived) => progress(&format!("Archived {}", archived.display())),
                Err(e) => progress(&format!("Book imported; the ACSM stays put: {e:#}")),
            }
            book
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
