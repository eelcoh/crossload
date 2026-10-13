//! Optional user defaults, separate from private activation/recovery state.
//!
//! The file lives in crossload's own settings folder, which the wiring hands
//! over as a `DirModify`; the paths in it are only names here. Which folders
//! they open is the wiring's business.
use ambit_caps_dir::{DirModify, RelPath};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Defaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// CrossPoint address used by send or an explicit --send-to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reader: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub browse: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// Send books to the Kobo as kepubs, which its own reading engine counts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kepub: Option<bool>,
    /// The reader model to prepare device copies for, as CrossPoint names it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// OPDS catalogue browsed by default, instead of the Palace Bookshelf.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
}
/// Where the settings file is: a file in crossload's own settings folder.
#[derive(Clone)]
pub struct Store {
    pub dir: std::sync::Arc<dyn DirModify>,
    pub file: RelPath,
    /// The file as the user knows it, for messages.
    pub display: String,
    /// The home directory, for `~/` in saved paths.
    pub home: Option<String>,
}

/// Read the settings: missing is empty, unreadable or invalid is an error.
pub fn load(store: &Store) -> Result<Defaults> {
    let bytes = match crate::files::read(&*store.dir, &store.file, 65536) {
        Ok(bytes) => bytes,
        // No settings folder yet (it is made on the first save) is no settings either.
        Err(e) if crate::files::missing(&e) || crate::files::unavailable(&e) => {
            return Ok(Defaults::default())
        }
        Err(e) => {
            return Err(e).with_context(|| format!("Cannot read configuration {}", store.display))
        }
    };
    let mut config: Defaults = serde_json::from_slice(&bytes)
        .with_context(|| format!("Invalid configuration at {}", store.display))?;
    config.normalize(store.home.as_deref())?;
    Ok(config)
}
impl Defaults {
    /// The screen device copies are made for. An unknown name is refused
    /// rather than quietly falling back to a screen the reader does not have.
    pub fn screen(&self) -> Result<crate::profile::Profile> {
        match self.profile.as_deref() {
            None => Ok(crate::profile::DEFAULT),
            Some(name) => crate::profile::for_device(name).with_context(|| {
                format!(
                    "Unknown reader {name}; known screens are {}. Leave profile unset for {}, \
                     or use --no-optimize to send books as they are",
                    crate::profile::known(),
                    crate::profile::DEFAULT.device
                )
            }),
        }
    }
    pub fn normalize(&mut self, home: Option<&str>) -> Result<()> {
        for value in [
            &mut self.device,
            &mut self.output,
            &mut self.copy_to,
            &mut self.browse,
        ]
        .into_iter()
        .flatten()
        {
            if value == "~" || value.starts_with("~/") {
                let home = home.context("HOME is unset")?.trim_end_matches('/');
                *value = format!("{home}{}", &value[1..]);
            }
            ensure!(
                value.starts_with('/'),
                "Configuration paths must be absolute (or start with ~/): {value}"
            );
        }
        crate::crosspoint::Address::parse(
            self.reader.as_deref().unwrap_or("crosspoint.local"),
            self.folder.as_deref().unwrap_or("/"),
        )?;
        // A catalogue that is not a usable address is refused at save time,
        // like the screen below, rather than on every browse afterwards.
        if let Some(catalog) = &self.catalog {
            let url = url::Url::parse(catalog)
                .with_context(|| format!("Invalid catalogue address {catalog}"))?;
            ensure!(
                matches!(url.scheme(), "http" | "https"),
                "A catalogue address must be http or https"
            );
        }
        // A screen nobody knows is refused here, when it is being saved,
        // rather than on every command that reads it afterwards.
        self.screen()?;
        Ok(())
    }
    pub fn merge(&mut self, update: Self) {
        if update.device.is_some() {
            self.device = update.device;
        }
        if update.output.is_some() {
            self.output = update.output;
        }
        if update.reader.is_some() {
            self.reader = update.reader;
        }
        if update.copy_to.is_some() {
            self.copy_to = update.copy_to;
        }
        if update.browse.is_some() {
            self.browse = update.browse;
        }
        if update.folder.is_some() {
            self.folder = update.folder;
        }
        if update.kepub.is_some() {
            self.kepub = update.kepub;
        }
        if update.profile.is_some() {
            self.profile = update.profile;
        }
        if update.catalog.is_some() {
            self.catalog = update.catalog;
        }
    }
    pub fn unset(&mut self, key: &str) -> Result<()> {
        match key {
            "device" => self.device = None,
            "output" => self.output = None,
            "reader" => self.reader = None,
            "copy-to" => self.copy_to = None,
            "browse" => self.browse = None,
            "folder" => self.folder = None,
            "kepub" => self.kepub = None,
            "profile" => self.profile = None,
            "catalog" => self.catalog = None,
            _ => anyhow::bail!(
                "Unknown setting {key}; use device, output, reader, copy-to, browse, folder, kepub, profile or catalog"
            ),
        }
        Ok(())
    }
    pub fn save(&mut self, store: &Store) -> Result<()> {
        self.normalize(store.home.as_deref())?;
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        store
            .dir
            .replace(&store.file, &bytes)
            .context("Cannot save configuration")?;
        Ok(())
    }
}
pub fn required<T>(explicit: Option<T>, saved: Option<T>, flag: &str) -> Result<T> {
    explicit.or(saved).with_context(|| {
        format!("Missing {flag}; supply it or save a default with crossload config set")
    })
}
