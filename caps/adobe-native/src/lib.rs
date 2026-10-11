//! [`Adept`] on libgourou: ACSM fulfilment with a privately stored, existing activation.
//!
//! Everything it keeps is in one state directory the wiring names: the activation and, per ACSM,
//! the recovery files of its fulfilment. The directory is `0700` and its files `0600`, and one
//! process at a time uses it (a lock taken per call, so the terminal interface and a command
//! line can take turns).
//!
//! The C++ (libgourou, pugixml, uPDFParser, libzip, and curl and OpenSSL for its HTTPS) is built by
//! `build.rs` and reached through one `extern "C"` entry point, `xteink_adobe`, which catches every
//! exception. Calls into it are serialized.
use crossload_adobe::{Activation, Adept, AdeptError};
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
// These crates supply the native link dependencies used by the C++ bridge.
use {curl_sys as _, libz_sys as _, openssl_sys as _};

const MAX_XML: u64 = 8 * 1024 * 1024;
/// The largest book this hands back: the app's own limit.
const MAX_BOOK: u64 = 128 * 1024 * 1024;
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

type Result<T> = std::result::Result<T, AdeptError>;

fn fail(message: impl Into<String>) -> AdeptError {
    AdeptError(message.into())
}

/// Adds what was being done to an error, as `anyhow`'s context did.
trait Context<T> {
    fn context(self, what: impl std::fmt::Display) -> Result<T>;
}

impl<T, E: std::fmt::Display> Context<T> for std::result::Result<T, E> {
    fn context(self, what: impl std::fmt::Display) -> Result<T> {
        self.map_err(|e| fail(format!("{what}: {e}")))
    }
}

fn ensure(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(fail(message))
    }
}

/// Where the activation lives unless the wiring says otherwise. Frozen: renaming it would
/// orphan the activation of everyone who installed under the old name.
pub fn default_state_dir(home: &Path, xdg_data_home: Option<&Path>) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/xteink")
    } else {
        xdg_data_home
            .filter(|p| p.is_absolute())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("xteink")
    }
}

fn private_dir(path: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .context(path.display())?;
    let meta = fs::symlink_metadata(path).context(path.display())?;
    ensure(
        meta.is_dir() && !meta.file_type().is_symlink(),
        "State directory must be a real directory",
    )?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).context(path.display())?;
    Ok(())
}

/// The state directory, created on first use.
pub struct NativeAdept {
    root: PathBuf,
}

/// The state directory, locked for one call.
struct Store {
    root: PathBuf,
    _lock: File,
}

impl NativeAdept {
    pub fn new(state: impl AsRef<Path>) -> Self {
        Self {
            root: state.as_ref().to_path_buf(),
        }
    }

    fn open(&self) -> Result<Store> {
        private_dir(&self.root)?;
        let lock_path = self.root.join(".lock");
        if let Ok(meta) = fs::symlink_metadata(&lock_path) {
            ensure(
                meta.is_file() && !meta.file_type().is_symlink(),
                "Invalid state lock file",
            )?;
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(lock_path)
            .context("Cannot open the state lock")?;
        lock.try_lock()
            .map_err(|_| fail("Another crossload process is using this state directory"))?;
        Ok(Store {
            root: self.root.canonicalize().context(self.root.display())?,
            _lock: lock,
        })
    }
}

/// Reads an activation export: a Calibre ACSM Input/libgourou activation directory, or a ZIP of
/// one. The desktop application's activation.dat alone is not this format.
pub fn read_activation(source: &Path) -> Result<Activation> {
    let mut zip = if source.is_file() {
        Some(zip::ZipArchive::new(File::open(source).context(source.display())?).map_err(|_| {
            fail("Expected an activation export ZIP or directory containing device.xml, activation.xml and devicesalt; desktop activation.dat is not supported")
        })?)
    } else {
        None
    };
    let mut files = Vec::new();
    for name in FILES {
        let data = if let Some(archive) = zip.as_mut() {
            let matches: Vec<_> = archive
                .file_names()
                .filter(|n| n.rsplit('/').next() == Some(name))
                .map(str::to_owned)
                .collect();
            ensure(
                matches.len() == 1,
                format!("Activation ZIP must contain exactly one {name}"),
            )?;
            let member = archive.by_name(&matches[0]).context(name)?;
            ensure(
                member.unix_mode().is_none_or(|m| m & 0o170000 != 0o120000),
                "Activation ZIP contains a symlink",
            )?;
            bounded(member, MAX_XML)?
        } else {
            read(&source.join(name), MAX_XML).map_err(|e| {
                fail(format!("Missing or unreadable {name}; import a Calibre ACSM Input/libgourou activation export: {e}"))
            })?
        };
        files.push(data);
    }
    let devicesalt = files.pop().expect("three files");
    let activation_xml = files.pop().expect("three files");
    let device_xml = files.pop().expect("three files");
    Ok(Activation {
        device_xml,
        activation_xml,
        devicesalt,
    })
}

impl Adept for NativeAdept {
    fn import_activation(&self, activation: &Activation) -> Result<()> {
        let store = self.open()?;
        let destination = store.root.join("activation");
        ensure(
            !destination.exists(),
            "An activation is already installed; use a separate --state-dir to import another",
        )?;
        let stage = tempfile::Builder::new()
            .prefix("activation-")
            .tempdir_in(&store.root)
            .context("Cannot stage the activation")?;
        for (name, data) in FILES.into_iter().zip([
            &activation.device_xml,
            &activation.activation_xml,
            &activation.devicesalt,
        ]) {
            if name == "devicesalt" {
                ensure(data.len() == 16, "devicesalt must contain exactly 16 bytes")?;
            } else {
                let text = std::str::from_utf8(data).context(format!("Invalid {name}"))?;
                roxmltree::Document::parse(text).context(format!("Invalid {name}"))?;
            }
            write_private(&stage.path().join(name), data)?;
        }
        native(0, stage.path(), Path::new(""), Path::new("")).map_err(|e| {
            fail(format!(
                "Activation validation failed; nothing was installed: {e}"
            ))
        })?;
        fs::rename(stage.path(), destination).context("Cannot install the activation")?;
        Ok(())
    }

    fn status(&self) -> Result<bool> {
        let store = self.open()?;
        if !store.root.join("activation").exists() {
            return Ok(false);
        }
        store.validate_activation()?;
        Ok(true)
    }

    fn fulfil(&self, data: &[u8]) -> Result<Vec<u8>> {
        ensure(data.len() as u64 <= MAX_XML, "ACSM exceeds 8 MiB")?;
        let xml =
            roxmltree::Document::parse(std::str::from_utf8(data).context("Invalid ACSM XML")?)
                .context("Invalid ACSM XML")?;
        ensure(
            xml.descendants()
                .any(|n| n.is_element() && n.tag_name().name() == "fulfillmentToken"),
            "Input is not an ACSM fulfillment token",
        )?;
        ensure(
            !xml.descendants().any(|n| {
                n.is_element()
                    && n.tag_name().name() == "format"
                    && n.text().is_some_and(|v| v.contains("application/pdf"))
            }),
            "PDF ACSM files are not supported yet",
        )?;
        let store = self.open()?;
        store.validate_activation()?;
        let id = format!("{:x}", Sha256::digest(data));
        let cache = store.root.join("acsm").join(&id);
        private_dir(&cache)?;
        let activation = store.root.join("activation");
        let receipt = cache.join("receipt.xml");
        if !receipt.exists() {
            let started = cache.join("fulfillment-started");
            ensure(!started.exists(), format!("An earlier fulfillment did not produce a receipt. Its server outcome is unknown; crossload will not repeat it automatically. Recovery files: {}", cache.display()))?;
            let input = cache.join("input.acsm");
            write_private(&input, data)?;
            write_private(&started, b"Fulfillment may have reached the server.\n")?;
            native(1, &activation, &input, &receipt).map_err(|e| {
                fail(format!(
                    "Fulfillment failed; request was not retried. Recovery files: {}: {e}",
                    cache.display()
                ))
            })?;
            fs::set_permissions(&receipt, fs::Permissions::from_mode(0o600))
                .context(receipt.display())?;
        }
        let original = cache.join("original.epub");
        if !original.exists() {
            let download = cache.join("download.part");
            native(2, &activation, &receipt, &download).map_err(|e| {
                fail(format!(
                    "Download failed; run the same command again to reuse the saved receipt at {}: {e}",
                    receipt.display()
                ))
            })?;
            let bytes = read(&download, MAX_BOOK)?;
            ensure(
                bytes.starts_with(b"PK\x03\x04"),
                "Downloaded file is not a supported EPUB; receipt retained",
            )?;
            fs::rename(download, &original).context(original.display())?;
        }
        let bytes = read(&original, MAX_BOOK)?;
        let clear = tempfile::Builder::new()
            .prefix("decrypt-")
            .tempfile_in(&cache)
            .context("Cannot stage decryption")?;
        fs::write(clear.path(), &bytes).context("Cannot stage decryption")?;
        native(3, &activation, &original, clear.path()).map_err(|e| {
            fail(format!(
                "Decryption failed; original download retained at {}: {e}",
                original.display()
            ))
        })?;
        read(clear.path(), MAX_BOOK)
    }
}

impl Store {
    fn validate_activation(&self) -> Result<()> {
        let activation = self.root.join("activation");
        for name in FILES {
            let data = read(&activation.join(name), MAX_XML).map_err(|_| {
                fail("No usable activation; run crossload adobe setup --from <activation directory or ZIP>")
            })?;
            if name == "devicesalt" {
                ensure(data.len() == 16, "Invalid devicesalt size")?;
            }
            fs::set_permissions(activation.join(name), fs::Permissions::from_mode(0o600))
                .context(name)?;
        }
        native(0, &activation, Path::new(""), Path::new(""))
    }
}

fn bounded(reader: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    reader
        .take(limit + 1)
        .read_to_end(&mut data)
        .context("Cannot read")?;
    ensure(
        data.len() as u64 <= limit,
        format!("File exceeds size limit ({limit} bytes)"),
    )?;
    Ok(data)
}

fn read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path).context(path.display())?;
    ensure(
        meta.is_file() && !meta.file_type().is_symlink(),
        format!("Expected a regular file: {}", path.display()),
    )?;
    bounded(File::open(path).context(path.display())?, limit)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .context(path.display())?;
    file.write_all(bytes).context(path.display())?;
    file.sync_all().context(path.display())?;
    Ok(())
}

fn native(operation: i32, activation: &Path, input: &Path, output: &Path) -> Result<()> {
    let _guard = NATIVE
        .lock()
        .map_err(|_| fail("Native backend lock poisoned"))?;
    let cpath = |p: &Path| CString::new(p.as_os_str().as_bytes()).context("Path contains NUL");
    let activation = cpath(activation)?;
    let input = cpath(input)?;
    let output = cpath(output)?;
    let certs = openssl_probe::probe();
    let cert_file = cpath(certs.cert_file.as_deref().unwrap_or(Path::new("")))?;
    let cert_dir = cpath(certs.cert_dir.as_deref().unwrap_or(Path::new("")))?;
    let mut error = [0 as std::ffi::c_char; 4096];
    // SAFETY: all arguments remain alive for this synchronous call. The C++ boundary catches
    // every exception and bounds/NUL-terminates its error.
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
        // SAFETY: the bridge NUL-terminates within `capacity`.
        let message = unsafe { CStr::from_ptr(error.as_ptr()) }.to_string_lossy();
        return Err(fail(format!("Native ADEPT backend: {message}")));
    }
    Ok(())
}
