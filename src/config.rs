//! Optional user defaults, separate from private activation/recovery state.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Default, Deserialize, Serialize, clap::Args)]
#[serde(default, deny_unknown_fields)]
pub struct Defaults {
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<PathBuf>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<PathBuf>,
    /// CrossPoint address used by send or an explicit --send-to.
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reader: Option<String>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy_to: Option<PathBuf>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub browse: Option<PathBuf>,
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}
pub fn default_path() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .context("HOME is unset; use --config <file>")?;
    Ok(base.join("crossload/config.json"))
}
pub fn load(path: &Path) -> Result<Defaults> {
    match fs::File::open(path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(65537).read_to_end(&mut bytes)?;
            ensure!(bytes.len() <= 65536, "Configuration exceeds 64 KiB");
            let mut config: Defaults = serde_json::from_slice(&bytes)
                .with_context(|| format!("Invalid configuration at {}", path.display()))?;
            config.normalize()?;
            Ok(config)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Defaults::default()),
        Err(e) => Err(e).with_context(|| format!("Cannot read configuration {}", path.display())),
    }
}
impl Defaults {
    pub fn normalize(&mut self) -> Result<()> {
        for value in [
            &mut self.device,
            &mut self.output,
            &mut self.copy_to,
            &mut self.browse,
        ]
        .into_iter()
        .flatten()
        {
            if let Ok(rest) = value.strip_prefix("~") {
                *value =
                    PathBuf::from(std::env::var_os("HOME").context("HOME is unset")?).join(rest);
            }
            ensure!(
                value.is_absolute(),
                "Configuration paths must be absolute (or start with ~/): {}",
                value.display()
            );
        }
        crate::crosspoint::Reader::new(
            self.reader.as_deref().unwrap_or("crosspoint.local"),
            self.folder.as_deref().unwrap_or("/"),
        )?;
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
    }
    pub fn unset(&mut self, key: &str) -> Result<()> {
        match key {
            "device" => self.device = None,
            "output" => self.output = None,
            "reader" => self.reader = None,
            "copy-to" => self.copy_to = None,
            "browse" => self.browse = None,
            "folder" => self.folder = None,
            _ => anyhow::bail!(
                "Unknown setting {key}; use device, output, reader, copy-to, browse or folder"
            ),
        }
        Ok(())
    }
    pub fn save(&mut self, path: &Path) -> Result<()> {
        self.normalize()?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(&serde_json::to_vec_pretty(self)?)?;
        file.write_all(b"\n")?;
        file.as_file().sync_all()?;
        file.persist(path).context("Cannot save configuration")?;
        Ok(())
    }
}
pub fn required<T>(explicit: Option<T>, saved: Option<T>, flag: &str) -> Result<T> {
    explicit.or(saved).with_context(|| {
        format!("Missing {flag}; supply it or save a default with crossload config set")
    })
}
