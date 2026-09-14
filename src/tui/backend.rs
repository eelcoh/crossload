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
pub(super) fn perform(options: Options, entry: Entry, progress: impl Fn(&str)) -> Result<String> {
    let path = match entry.source {
        Source::Book(book, target) => {
            return crate::books::transfer(&library_options(&options), &book, target, &progress)
                .map(|message| format!("{message} Press r to refresh."))
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
