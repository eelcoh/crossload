//! Native ACSM fulfillment using a privately stored, existing ADEPT activation.
use crate::epub;
use anyhow::{bail, ensure, Context, Result};
use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::{
    ffi::{CStr, CString},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::Mutex,
};
use tempfile::NamedTempFile;
// These crates supply the native link dependencies used by the C++ bridge.
use {curl_sys as _, libz_sys as _, openssl_sys as _};

const MAX_XML: u64 = 8 * 1024 * 1024;
const FILES: [&str; 3] = ["device.xml", "activation.xml", "devicesalt"];
static NATIVE: Mutex<()> = Mutex::new(());
unsafe extern "C" {
    fn xteink_adobe(
        operation: i32,
        activation: *const std::ffi::c_char,
        input: *const std::ffi::c_char,
        output: *const std::ffi::c_char,
        ca_file: *const std::ffi::c_char,
        ca_dir: *const std::ffi::c_char,
        error: *mut std::ffi::c_char,
        capacity: usize,
    ) -> i32;
}

pub fn default_state_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is unset; use --state-dir")?;
    if cfg!(target_os = "macos") {
        Ok(PathBuf::from(home).join("Library/Application Support/xteink"))
    } else {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| PathBuf::from(home).join(".local/share"));
        Ok(base.join("xteink"))
    }
}

fn private_dir(path: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.is_dir() && !meta.file_type().is_symlink(),
        "State directory must be a real directory"
    );
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

pub struct Store {
    root: PathBuf,
    _lock: File,
}
impl Store {
    pub fn open(root: &Path) -> Result<Self> {
        private_dir(root)?;
        let lock_path = root.join(".lock");
        if let Ok(meta) = fs::symlink_metadata(&lock_path) {
            ensure!(
                meta.is_file() && !meta.file_type().is_symlink(),
                "Invalid state lock file"
            );
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(lock_path)?;
        lock.try_lock_exclusive()
            .context("Another xteink process is using this state directory")?;
        Ok(Self {
            root: root.canonicalize()?,
            _lock: lock,
        })
    }

    /// Import a Calibre ACSM Input/libgourou activation directory or export ZIP.
    /// The desktop application's activation.dat alone is not this format.
    pub fn import_activation(&self, source: &Path) -> Result<()> {
        let destination = self.root.join("activation");
        ensure!(
            !destination.exists(),
            "An activation is already installed; use a separate --state-dir to import another"
        );
        let stage = tempfile::Builder::new()
            .prefix("activation-")
            .tempdir_in(&self.root)?;
        let mut zip = if source.is_file() {
            Some(zip::ZipArchive::new(File::open(source)?).context("Expected an activation export ZIP or directory containing device.xml, activation.xml and devicesalt; desktop activation.dat is not supported")?)
        } else {
            None
        };
        for name in FILES {
            let data = if let Some(archive) = zip.as_mut() {
                let matches: Vec<_> = archive
                    .file_names()
                    .filter(|n| n.rsplit('/').next() == Some(name))
                    .map(str::to_owned)
                    .collect();
                ensure!(
                    matches.len() == 1,
                    "Activation ZIP must contain exactly one {name}"
                );
                let member = archive.by_name(&matches[0])?;
                ensure!(
                    member.unix_mode().is_none_or(|m| m & 0o170000 != 0o120000),
                    "Activation ZIP contains a symlink"
                );
                bounded(member, MAX_XML)?
            } else {
                read(&source.join(name), MAX_XML).with_context(|| format!("Missing or unreadable {name}; import a Calibre ACSM Input/libgourou activation export"))?
            };
            if name == "devicesalt" {
                ensure!(data.len() == 16, "devicesalt must contain exactly 16 bytes");
            } else {
                roxmltree::Document::parse(std::str::from_utf8(&data)?)
                    .with_context(|| format!("Invalid {name}"))?;
            }
            write_private(&stage.path().join(name), &data)?;
        }
        native(0, stage.path(), Path::new(""), Path::new(""))
            .context("Activation validation failed; nothing was installed")?;
        fs::rename(stage.path(), destination)?;
        Ok(())
    }

    pub fn status(&self) -> Result<bool> {
        let activation = self.root.join("activation");
        if !activation.exists() {
            return Ok(false);
        }
        self.validate_activation()?;
        Ok(true)
    }

    fn validate_activation(&self) -> Result<()> {
        let activation = self.root.join("activation");
        for name in FILES {
            let data = read(&activation.join(name), MAX_XML).context(
                "No usable activation; run crossload adobe setup --from <activation directory or ZIP>",
            )?;
            if name == "devicesalt" {
                ensure!(data.len() == 16, "Invalid devicesalt size");
            }
            fs::set_permissions(activation.join(name), fs::Permissions::from_mode(0o600))?;
        }
        native(0, &activation, Path::new(""), Path::new(""))
    }

    /// Move a spent ACSM into an `archive/` folder beside it, so a fulfilled
    /// request stops appearing as a pending one. Nothing is ever deleted or
    /// overwritten: a name already taken gains a suffix. Call it only after a
    /// fulfillment that produced a book; an uncertain outcome must stay visible.
    pub fn archive(acsm: &Path) -> Result<PathBuf> {
        let directory = acsm
            .parent()
            .context("ACSM has no containing directory")?
            .join("archive");
        fs::create_dir_all(&directory)?;
        let name = acsm.file_name().context("ACSM has no file name")?;
        let stem = Path::new(name)
            .file_stem()
            .unwrap_or(name)
            .to_string_lossy()
            .into_owned();
        let extension = Path::new(name)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        for attempt in 0..1000 {
            let target = directory.join(if attempt == 0 {
                format!("{stem}{extension}")
            } else {
                format!("{stem}-{attempt}{extension}")
            });
            // Claim the name before moving onto it, so a second run cannot
            // replace an ACSM that is already archived.
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
            {
                Ok(_) => {
                    fs::rename(acsm, &target)?;
                    return Ok(target);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        anyhow::bail!("Too many archived copies of {stem}{extension}")
    }
    pub fn import(&self, acsm: &Path, output: &Path) -> Result<PathBuf> {
        let data = read(acsm, MAX_XML)?;
        let xml =
            roxmltree::Document::parse(std::str::from_utf8(&data)?).context("Invalid ACSM XML")?;
        ensure!(
            xml.descendants()
                .any(|n| n.is_element() && n.tag_name().name() == "fulfillmentToken"),
            "Input is not an ACSM fulfillment token"
        );
        ensure!(
            !xml.descendants().any(|n| n.is_element()
                && n.tag_name().name() == "format"
                && n.text().is_some_and(|v| v.contains("application/pdf"))),
            "PDF ACSM files are not supported yet"
        );
        self.validate_activation()?;
        fs::create_dir_all(output)?;
        let output = output.canonicalize()?;
        ensure!(
            !output.starts_with(&self.root),
            "Choose an output directory outside private xteink state"
        );
        // Check destination writability before making any fulfillment requests.
        let mut publication = NamedTempFile::new_in(&output)?;
        let id = format!("{:x}", Sha256::digest(&data));
        let cache = self.root.join("acsm").join(&id);
        private_dir(&cache)?;
        let activation = self.root.join("activation");
        let receipt = cache.join("receipt.xml");
        if !receipt.exists() {
            let started = cache.join("fulfillment-started");
            ensure!(!started.exists(), "An earlier fulfillment did not produce a receipt. Its server outcome is unknown; xteink will not repeat it automatically. Recovery files: {}", cache.display());
            let input = cache.join("input.acsm");
            write_private(&input, &data)?;
            write_private(&started, b"Fulfillment may have reached the server.\n")?;
            native(1, &activation, &input, &receipt).with_context(|| {
                format!(
                    "Fulfillment failed; request was not retried. Recovery files: {}",
                    cache.display()
                )
            })?;
            fs::set_permissions(&receipt, fs::Permissions::from_mode(0o600))?;
        }
        let original = cache.join("original.epub");
        if !original.exists() {
            let download = cache.join("download.part");
            native(2, &activation, &receipt, &download).with_context(|| {
                format!(
                    "Download failed; run the same command again to reuse the saved receipt at {}",
                    receipt.display()
                )
            })?;
            let bytes = read(&download, epub::MAX_BOOK_BYTES)?;
            epub::inspect(&bytes)
                .context("Downloaded file is not a supported EPUB; receipt retained")?;
            fs::rename(download, &original)?;
        }
        let bytes = read(&original, epub::MAX_BOOK_BYTES)?;
        epub::inspect(&bytes)?;
        let clear = tempfile::Builder::new()
            .prefix("decrypt-")
            .tempfile_in(&cache)?;
        fs::write(clear.path(), &bytes)?;
        native(3, &activation, &original, clear.path()).with_context(|| {
            format!(
                "Decryption failed; original download retained at {}",
                original.display()
            )
        })?;
        let bytes = read(clear.path(), epub::MAX_BOOK_BYTES)?;
        epub::font_metadata(&bytes).context("Unsupported encryption remains after decryption")?;
        epub::validate(&bytes)
            .context("Imported book failed EPUB validation; no output was published")?;
        let metadata = epub::metadata(clear.path())?.context("Missing EPUB metadata")?;
        let name = output_name(metadata.title.as_deref().unwrap_or("Book"), &id);
        let target = output.join(name);
        publication.write_all(&bytes)?;
        publication.as_file().sync_all()?;
        publication.persist_noclobber(&target).with_context(|| {
            format!(
                "Cannot publish {}; existing files are never overwritten. To transfer the existing local EPUB, use crossload send instead of importing again",
                target.display()
            )
        })?;
        Ok(target)
    }
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
fn bounded(reader: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    reader.take(limit + 1).read_to_end(&mut data)?;
    ensure!(
        data.len() as u64 <= limit,
        "File exceeds size limit ({limit} bytes)"
    );
    Ok(data)
}
fn read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.is_file() && !meta.file_type().is_symlink(),
        "Expected a regular file: {}",
        path.display()
    );
    bounded(File::open(path)?, limit)
}
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn native(operation: i32, activation: &Path, input: &Path, output: &Path) -> Result<()> {
    let _guard = NATIVE
        .lock()
        .map_err(|_| anyhow::anyhow!("Native backend lock poisoned"))?;
    let cpath = |p: &Path| CString::new(p.as_os_str().as_bytes()).context("Path contains NUL");
    let activation = cpath(activation)?;
    let input = cpath(input)?;
    let output = cpath(output)?;
    let certs = openssl_probe::probe();
    let cert_file = cpath(certs.cert_file.as_deref().unwrap_or(Path::new("")))?;
    let cert_dir = cpath(certs.cert_dir.as_deref().unwrap_or(Path::new("")))?;
    let mut error = [0_i8; 4096];
    // SAFETY: all arguments remain alive for this synchronous call. The C++
    // boundary catches every exception and bounds/NUL-terminates its error.
    let code = unsafe {
        xteink_adobe(
            operation,
            activation.as_ptr(),
            input.as_ptr(),
            output.as_ptr(),
            cert_file.as_ptr(),
            cert_dir.as_ptr(),
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if code != 0 {
        let message = unsafe { CStr::from_ptr(error.as_ptr()) }.to_string_lossy();
        bail!("Native ADEPT backend: {message}");
    }
    Ok(())
}
