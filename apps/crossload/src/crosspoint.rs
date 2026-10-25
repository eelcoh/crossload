//! CrossPoint 1.6.0 HTTP transfer protocol (upstream tag 54337e6d).
//!
//! The reader is reached only through capabilities: an `Http` for its small
//! requests, another for whole files (a larger body limit and timeout), and a
//! `WebSocket` for uploads. Their scopes are the wiring's.
use std::{sync::Arc, time::Duration};

use ambit_caps::{Clock, Random};
use ambit_caps_net::{http, Http, HttpErrorKind, WebSocket, WsConnection, WsMessage};
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use url::Url;

use crate::{epub, format, format::Format};

const MAX_RESPONSE: usize = 4 * 1024 * 1024;
/// How long one upload may take, start to finish.
const UPLOAD_DEADLINE: Duration = Duration::from_secs(600);

#[derive(Debug, Deserialize)]
pub struct Status {
    pub version: String,
    pub device: String,
    /// Sent from 2.0 on, which also names devices differently (`xteink_x4_pro`).
    #[serde(rename = "firmwareFamily", default)]
    family: Option<String>,
}

#[derive(Deserialize)]
struct Entry {
    name: String,
    size: u64,
    #[serde(rename = "isDirectory")]
    directory: bool,
}

#[derive(Debug)]
pub struct Sent {
    pub path: String,
    pub bytes: usize,
    pub already_present: bool,
    pub backup: Option<String>,
    /// Something worth saying that is not a failure: the reader is not the
    /// screen the book was prepared for.
    pub note: Option<String>,
}

/// The address shown in File Transfer mode, and the folder on the reader's
/// card that books go into. Checked, but nothing is contacted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    base: Url,
    folder: String,
}

impl Address {
    /// Accept the address shown in File Transfer mode, including bare IPs.
    pub fn parse(address: &str, folder: &str) -> Result<Self> {
        let address = address.trim();
        ensure!(
            !address.is_empty(),
            "Supply the address shown in CrossPoint File Transfer mode"
        );
        let base = Url::parse(&if address.contains("://") {
            address.to_owned()
        } else {
            format!("http://{address}")
        })
        .context("Invalid reader address")?;
        ensure!(
            base.scheme() == "http" && base.host_str().is_some(),
            "CrossPoint File Transfer requires an http:// address"
        );
        ensure!(
            base.username().is_empty()
                && base.password().is_none()
                && base.query().is_none()
                && base.fragment().is_none()
                && base.path() == "/",
            "Use only the reader's address, without credentials, a path, query, or fragment"
        );
        let folder = folder.trim_end_matches('/');
        let folder = if folder.is_empty() { "/" } else { folder };
        ensure!(folder.starts_with('/'), "Remote folder must start with /");
        if folder != "/" {
            for component in folder[1..].split('/') {
                valid_component(component)?;
            }
        }
        Ok(Self {
            base,
            folder: folder.to_owned(),
        })
    }

    /// The host, as typed: an IP address or a name.
    pub fn host(&self) -> &str {
        self.base
            .host_str()
            .unwrap_or_default()
            .trim_matches(['[', ']'])
    }

    pub fn port(&self) -> u16 {
        self.base.port_or_known_default().unwrap_or(80)
    }

    pub fn folder(&self) -> &str {
        &self.folder
    }
}

/// How the reader is reached.
#[derive(Clone)]
pub struct Link {
    /// Status, listings, folders and renames: small answers, short timeout.
    pub control: Arc<dyn Http>,
    /// Whole files, for inventory and the read-back after an upload.
    pub files: Arc<dyn Http>,
    pub upload: Arc<dyn WebSocket>,
    pub clock: Arc<dyn Clock>,
    /// Names for uploads in progress, so two clients never share one.
    pub random: Arc<dyn Random>,
    /// The upload WebSocket's port: 81 on CrossPoint.
    pub websocket_port: u16,
}

#[derive(Clone)]
pub struct Reader {
    /// The screen the books being sent were prepared for, so the reader can
    /// say if it is not that one.
    made_for: Option<crate::profile::Profile>,
    address: Address,
    link: Link,
    repair: bool,
}

impl Reader {
    pub fn new(address: Address, link: Link) -> Self {
        Self {
            made_for: None,
            address,
            link,
            repair: false,
        }
    }

    pub fn address(&self) -> &Address {
        &self.address
    }

    /// Say which screen the books being sent were prepared for, so the reader
    /// can be asked whether that is the screen it has.
    pub fn prepared_for(mut self, profile: Option<crate::profile::Profile>) -> Self {
        self.made_for = profile;
        self
    }
    pub fn repair_incomplete(mut self, repair: bool) -> Self {
        self.repair = repair;
        self
    }

    pub fn status(&self) -> Result<Status> {
        let data = self.request("/api/status", None, None, MAX_RESPONSE)?;
        let status: Status = serde_json::from_slice(&data)
            .context("Reader did not return CrossPoint status JSON")?;
        ensure!(
            !status.version.is_empty()
                && (status.family.as_deref() == Some("crosspoint")
                    || matches!(status.device.as_str(), "X4" | "X3")),
            "Unrecognized CrossPoint reader status"
        );
        Ok(status)
    }

    fn entries(&self, folder: &str) -> Result<Vec<Entry>> {
        serde_json::from_slice(&self.request("/api/files", Some(folder), None, MAX_RESPONSE)?)
            .context("Reader did not return a valid directory listing")
    }

    fn check_folder(&self) -> Result<()> {
        // CrossPoint returns an empty listing even for a missing directory.
        // Walk the parents so a miss is reported before starting an upload.
        let mut parent = String::from("/");
        for component in self.address.folder.split('/').filter(|s| !s.is_empty()) {
            ensure!(self.entries(&parent)?.iter().any(|e| e.directory && e.name == component), "Remote folder {} does not exist; create it in CrossPoint's file manager or omit --folder to use the SD-card root", self.address.folder);
            if parent != "/" {
                parent.push('/');
            }
            parent.push_str(component);
        }
        Ok(())
    }

    /// Read a bounded file for inventory/verification. No writes or retries.
    pub fn read_file(&self, path: &str, expected_size: u64) -> Result<Vec<u8>> {
        ensure!(
            expected_size <= epub::MAX_BOOK_BYTES,
            "Reader file exceeds 128 MiB: {path}"
        );
        let bytes = self.request("/download", Some(path), None, expected_size as usize)?;
        ensure!(
            bytes.len() as u64 == expected_size,
            "Reader file changed or download is incomplete: {path}"
        );
        Ok(bytes)
    }

    pub fn folder(&self) -> &str {
        &self.address.folder
    }

    /// Snapshot ordinary directory entries within the selected base folder.
    pub fn files(&self) -> Result<Vec<crate::inventory::FileEntry>> {
        self.status()?;
        self.check_folder()?;
        let mut pending = vec![(self.address.folder.clone(), 0)];
        let mut files = Vec::new();
        while let Some((folder, depth)) = pending.pop() {
            ensure!(depth <= 32, "Reader directory tree is too deep");
            for entry in self.entries(&folder)? {
                if entry.name.starts_with('.')
                    || matches!(
                        entry.name.to_lowercase().as_str(),
                        "xtcache" | "system volume information"
                    )
                {
                    continue;
                }
                valid_component(&entry.name)?;
                let path = format!("{}/{}", folder.trim_end_matches('/'), entry.name);
                if entry.directory {
                    pending.push((path.clone(), depth + 1));
                }
                files.push(crate::inventory::FileEntry {
                    path,
                    size: entry.size,
                    directory: entry.directory,
                });
                ensure!(
                    files.len() <= 20000,
                    "Reader inventory exceeds 20000 entries"
                );
            }
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }

    /// Create an author directory below the explicitly selected base folder.
    pub fn for_author(&self, author: &str) -> Result<Self> {
        valid_component(author)?;
        self.status()?;
        self.check_folder()?;
        let entries = self.entries(&self.address.folder)?;
        let name = if let Some(entry) = entries
            .iter()
            .find(|e| e.name.to_lowercase() == author.to_lowercase())
        {
            ensure!(entry.directory, "Author directory is occupied by a file");
            entry.name.clone()
        } else {
            let form = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("path", &self.address.folder)
                .append_pair("name", author)
                .finish();
            self.request("/mkdir", None, Some(form), MAX_RESPONSE)?;
            author.to_owned()
        };
        let mut reader = self.clone();
        reader.address.folder = format!("{}/{}", self.address.folder.trim_end_matches('/'), name);
        Ok(reader)
    }

    /// Upload `data` as `name` into the folder, verify the reader's copy, and
    /// only then give it its name. Nothing on the reader is ever overwritten.
    pub fn send(&self, name: &str, data: &[u8]) -> Result<Sent> {
        self.send_with_progress(name, data, &|_| {})
    }

    /// [`Reader::send`], saying how far it has come: each phase, and the bytes
    /// uploaded so far.
    pub fn send_with_progress(
        &self,
        name: &str,
        data: &[u8],
        progress: &dyn Fn(&str),
    ) -> Result<Sent> {
        valid_component(name)?;
        let kind = Format::of(name).context("Send requires an .epub, .pdf or .cbz file")?;
        ensure!(
            kind.shown_on_reader(),
            "CrossPoint stores a {} but its library never lists one; nothing was sent",
            kind.label()
        );
        ensure!(
            data.len() as u64 <= epub::MAX_BOOK_BYTES,
            "Book exceeds the 128 MiB transfer limit"
        );
        format::validate(kind, data).context("Book failed validation; nothing was sent")?;
        progress("Asking the reader for its status…");
        let status = self.status().context(
            "Cannot reach CrossPoint; open File Transfer and use the address shown on the reader",
        )?;
        // The reader gets to contradict what it was prepared for. This is the
        // only moment its name is known for free, and a device copy sized for
        // another screen still opens, which is what makes it worth saying.
        // Readers that share a screen share their copies, so only the screen counts.
        let reader = crate::profile::for_device(&status.device);
        let note = self
            .made_for
            .filter(|made_for| reader.is_none_or(|reader| reader.marker != made_for.marker))
            .map(|made_for| match reader {
                Some(reader) => format!(
                    "Note: this book was prepared for a {} screen ({}×{}), but the reader says it is \
                     a {} ({}×{}). Set profile to {}, or use --no-optimize to send books as they are.",
                    made_for.device,
                    made_for.width,
                    made_for.height,
                    reader.device,
                    reader.width,
                    reader.height,
                    reader.device
                ),
                None => format!(
                    "Note: this book was prepared for a {} screen ({}×{}), but the reader says it is \
                     a {}, whose screen is not known. Use --no-optimize to send books as they are.",
                    made_for.device, made_for.width, made_for.height, status.device
                ),
            });
        self.check_folder()?;
        let remote = format!("{}/{}", self.address.folder.trim_end_matches('/'), name);
        let entries = self.entries(&self.address.folder)?;
        let digest = format!("{:x}", Sha256::digest(data));
        let mut backup = None;
        // Repairs require a strictly shorter, byte-for-byte prefix. Unrelated
        // same-name books are never renamed or overwritten.
        if let Some(existing) = entries
            .iter()
            .find(|e| e.name.to_lowercase() == name.to_lowercase())
        {
            ensure!(
                !existing.directory,
                "A directory already exists at {remote}"
            );
            let existing_path = format!(
                "{}/{}",
                self.address.folder.trim_end_matches('/'),
                existing.name
            );
            if existing.size == data.len() as u64 {
                self.verify(&existing_path, data)
                    .context("A file with this name already exists; nothing was overwritten")?;
                return Ok(Sent {
                    path: existing_path,
                    bytes: data.len(),
                    already_present: true,
                    backup: None,
                    note,
                });
            }
            ensure!(self.repair && existing.size < data.len() as u64,
                "Reader has {} of {} bytes at {remote}. Nothing was overwritten. For a failed transfer, retry with crossload send --repair; it only repairs a verified incomplete prefix", existing.size, data.len());
            let partial = self.request(
                "/download",
                Some(&existing_path),
                None,
                existing.size as usize,
            )?;
            ensure!(
                partial.len() as u64 == existing.size && data.starts_with(&partial),
                "Existing file is not an incomplete prefix of this EPUB; repair refused"
            );
            let backup_name = format!("xteink-{}.incomplete-{}", &digest[..24], existing.size);
            self.rename(&existing_path, &backup_name)
                .context("Could not preserve incomplete file; no replacement was attempted")?;
            backup = Some(format!(
                "{}/{}",
                self.address.folder.trim_end_matches('/'),
                backup_name
            ));
        }
        // A random staging filename avoids taking over another client's upload
        // and is deliberately not .epub. The final name only appears after a
        // verified readback. Failed stages can be inspected without data loss.
        let staging_name = format!("xteink-{:016x}.uploading", self.link.random.u64());
        let staging = format!(
            "{}/{}",
            self.address.folder.trim_end_matches('/'),
            staging_name
        );
        self.upload(&staging_name, data, progress).with_context(|| format!(
            "Upload failed. No completed EPUB was published. Temporary path: {staging}. The local book is unchanged.{} Check SD-card free space and health if writes keep failing",
            backup.as_ref().map(|p| format!(" Incomplete original preserved at {p}." )).unwrap_or_default()))?;
        progress("Verifying the reader's copy…");
        self.verify(&staging, data).with_context(|| format!("Upload verification failed; not published as an EPUB. Temporary path: {staging}; local EPUB is unchanged"))?;
        self.rename(&staging, name).with_context(|| format!("Verified upload remains at {staging}; could not publish {remote}. Nothing was overwritten"))?;
        Ok(Sent {
            path: remote,
            bytes: data.len(),
            already_present: false,
            backup,
            note,
        })
    }

    fn rename(&self, path: &str, name: &str) -> Result<()> {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("path", path)
            .append_pair("name", name)
            .finish();
        let response = self.request("/rename", None, Some(body), MAX_RESPONSE)?;
        ensure!(
            response == b"Renamed successfully",
            "Reader did not confirm rename"
        );
        Ok(())
    }

    fn upload(&self, name: &str, data: &[u8], progress: &dyn Fn(&str)) -> Result<()> {
        let clock = &self.link.clock;
        let deadline = clock.elapsed() + UPLOAD_DEADLINE;
        let host = self.address.host();
        let host = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.to_owned()
        };
        let url = format!("ws://{host}:{}/", self.link.websocket_port);
        let mut ws = self.link.upload.connect(&url).map_err(|e| {
            anyhow::anyhow!(
                "Cannot connect to CrossPoint WebSocket port; keep File Transfer open (default port 81): {e}"
            )
        })?;
        let ws = &mut *ws;
        let result = (|| -> Result<()> {
            ws.send(WsMessage::Text(format!(
                "START:{name}:{}:{}",
                data.len(),
                self.address.folder
            )))?;
            ensure!(
                control(ws)? == "READY",
                "Reader did not acknowledge upload start"
            );
            let mut sent = 0;
            let total = crate::books::size(data.len() as u64);
            progress(&format!("Uploading to the reader… 0 B of {total}"));
            // Match CrossPoint's browser's 4 KiB frames, but wait for its 64 KiB
            // acknowledgements so network buffering cannot outrun the SD writer.
            for window in data.chunks(65536) {
                for chunk in window.chunks(4096) {
                    ensure!(clock.elapsed() < deadline, "Upload timed out");
                    ws.send(WsMessage::Binary(chunk.to_vec()))?;
                }
                sent += window.len();
                let expected = format!("PROGRESS:{sent}:{}", data.len());
                ensure!(
                    control(ws)? == expected,
                    "Reader acknowledged an unexpected byte count"
                );
                progress(&format!(
                    "Uploading to the reader… {} of {total}",
                    crate::books::size(sent as u64)
                ));
            }
            ensure!(
                control(ws)? == "DONE",
                "Reader did not confirm upload completion"
            );
            Ok(())
        })();
        ws.close();
        result
    }

    fn verify(&self, remote: &str, data: &[u8]) -> Result<()> {
        let copy = self.request("/download", Some(remote), None, data.len())?;
        ensure!(
            copy.len() == data.len() && Sha256::digest(&copy) == Sha256::digest(data),
            "Reader's copy differs from the local EPUB"
        );
        Ok(())
    }

    fn request(
        &self,
        endpoint: &str,
        path: Option<&str>,
        form: Option<String>,
        limit: usize,
    ) -> Result<Vec<u8>> {
        let mut url = self.address.base.join(endpoint)?;
        if let Some(path) = path {
            url.query_pairs_mut().append_pair("path", path);
        }
        // Whole files go through the capability made for them.
        let http = if endpoint == "/download" {
            &self.link.files
        } else {
            &self.link.control
        };
        let request = http::Request::builder().uri(url.as_str()).header(
            "User-Agent",
            concat!("crossload/", env!("CARGO_PKG_VERSION")),
        );
        let request = match form {
            Some(body) => request
                .method("POST")
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(body.into_bytes()),
            None => request.method("GET").body(Vec::new()),
        }
        .context("Invalid reader request")?;
        let response = http.send(request).map_err(|e| match e.kind {
            HttpErrorKind::Denied => anyhow::anyhow!(
                "{} is outside what this session may reach: a reader on the local network by IP \
                 address, or the saved reader by its name. Save the address in settings so \
                 crossload starts again with it",
                self.address.base
            ),
            HttpErrorKind::TooLarge => {
                anyhow::anyhow!("Reader response exceeded the expected size limit")
            }
            _ => anyhow::Error::new(e).context(format!("CrossPoint request failed: {endpoint}")),
        })?;
        let status = response.status().as_u16();
        let response = response.into_body();
        ensure!(
            response.len() <= limit,
            "Reader response exceeded the expected size limit"
        );
        ensure!(
            status == 200,
            "CrossPoint {endpoint} returned HTTP {status}: {}",
            String::from_utf8_lossy(&response)
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .take(300)
                .collect::<String>()
        );
        Ok(response)
    }
}

fn valid_component(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && !name.starts_with('.')
            && !name.ends_with([' ', '.'])
            && name.len() <= 255
            && !name
                .chars()
                .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
            && !matches!(
                name.to_lowercase().as_str(),
                "system volume information" | "xtcache"
            ),
        "Unsupported remote filename or folder component: {name:?}"
    );
    Ok(())
}

/// The reader's next word on the upload. Pings are the capability's to answer.
fn control(ws: &mut dyn WsConnection) -> Result<String> {
    match ws.recv() {
        Ok(WsMessage::Text(text)) => {
            ensure!(!text.starts_with("ERROR:"), "CrossPoint {}", text);
            Ok(text)
        }
        Ok(WsMessage::Binary(_)) => anyhow::bail!("Unexpected CrossPoint upload response"),
        Err(e) if e.kind == HttpErrorKind::Timeout => {
            anyhow::bail!("Upload timed out waiting for reader acknowledgement")
        }
        Err(e) => Err(anyhow::Error::new(e).context("CrossPoint upload connection interrupted")),
    }
}
