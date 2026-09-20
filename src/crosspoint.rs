//! CrossPoint 1.6.0 HTTP transfer protocol (upstream tag 54337e6d).
use std::{
    fs::File,
    io::Read,
    net::{TcpStream, ToSocketAddrs},
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{ensure, Context, Result};
use curl::easy::{Easy, List};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tungstenite::{protocol::WebSocketConfig, Message, WebSocket};
use url::Url;

use crate::{epub, format, format::Format};

const MAX_RESPONSE: usize = 4 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct Status {
    pub version: String,
    pub device: String,
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
}

#[derive(Clone)]
pub struct Reader {
    base: Url,
    folder: String,
    websocket_port: u16,
    repair: bool,
}

impl Reader {
    /// Accept the address shown in File Transfer mode, including bare IPs.
    pub fn new(address: &str, folder: &str) -> Result<Self> {
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
            websocket_port: 81,
            repair: false,
        })
    }

    pub fn with_websocket_port(mut self, port: u16) -> Self {
        self.websocket_port = port;
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
            !status.version.is_empty() && matches!(status.device.as_str(), "X4" | "X3"),
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
        for component in self.folder.split('/').filter(|s| !s.is_empty()) {
            ensure!(self.entries(&parent)?.iter().any(|e| e.directory && e.name == component), "Remote folder {} does not exist; create it in CrossPoint's file manager or omit --folder to use the SD-card root", self.folder);
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
        &self.folder
    }

    /// Snapshot ordinary directory entries within the selected base folder.
    pub fn files(&self) -> Result<Vec<crate::inventory::FileEntry>> {
        self.status()?;
        self.check_folder()?;
        let mut pending = vec![(self.folder.clone(), 0)];
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
        let entries = self.entries(&self.folder)?;
        let name = if let Some(entry) = entries
            .iter()
            .find(|e| e.name.to_lowercase() == author.to_lowercase())
        {
            ensure!(entry.directory, "Author directory is occupied by a file");
            entry.name.clone()
        } else {
            let form = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("path", &self.folder)
                .append_pair("name", author)
                .finish();
            self.request("/mkdir", None, Some(form), MAX_RESPONSE)?;
            author.to_owned()
        };
        let mut reader = self.clone();
        reader.folder = format!("{}/{}", self.folder.trim_end_matches('/'), name);
        Ok(reader)
    }

    pub fn send(&self, book: &Path) -> Result<Sent> {
        let name = book
            .file_name()
            .and_then(|s| s.to_str())
            .context("Book filename must be UTF-8")?;
        valid_component(name)?;
        let kind = Format::of(name).context("Send requires an .epub, .pdf or .cbz file")?;
        ensure!(
            kind.shown_on_reader(),
            "CrossPoint stores a {} but its library never lists one; nothing was sent",
            kind.label()
        );
        let file = File::open(book).context("Cannot open the book to send")?;
        ensure!(file.metadata()?.is_file(), "Send requires a regular file");
        let mut data = Vec::new();
        file.take(epub::MAX_BOOK_BYTES + 1).read_to_end(&mut data)?;
        ensure!(
            data.len() as u64 <= epub::MAX_BOOK_BYTES,
            "Book exceeds the 128 MiB transfer limit"
        );
        format::validate(kind, &data).context("Book failed validation; nothing was sent")?;
        self.status().context(
            "Cannot reach CrossPoint; open File Transfer and use the address shown on the reader",
        )?;
        self.check_folder()?;
        let remote = format!("{}/{}", self.folder.trim_end_matches('/'), name);
        let entries = self.entries(&self.folder)?;
        let digest = format!("{:x}", Sha256::digest(&data));
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
            let existing_path = format!("{}/{}", self.folder.trim_end_matches('/'), existing.name);
            if existing.size == data.len() as u64 {
                self.verify(&existing_path, &data)
                    .context("A file with this name already exists; nothing was overwritten")?;
                return Ok(Sent {
                    path: existing_path,
                    bytes: data.len(),
                    already_present: true,
                    backup: None,
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
                self.folder.trim_end_matches('/'),
                backup_name
            ));
        }
        // A random staging filename avoids taking over another client's upload
        // and is deliberately not .epub. The final name only appears after a
        // verified readback. Failed stages can be inspected without data loss.
        let staging_id = tempfile::Builder::new().prefix("xteink-").tempfile()?;
        let staging_name = format!(
            "{}.uploading",
            staging_id.path().file_name().unwrap().to_string_lossy()
        );
        let staging = format!("{}/{}", self.folder.trim_end_matches('/'), staging_name);
        self.upload(&staging_name, &data).with_context(|| format!(
            "Upload failed. No completed EPUB was published. Temporary path: {staging}. Local EPUB retained at {}.{} Check SD-card free space and health if writes keep failing",
            book.display(), backup.as_ref().map(|p| format!(" Incomplete original preserved at {p}." )).unwrap_or_default()))?;
        self.verify(&staging, &data).with_context(|| format!("Upload verification failed; not published as an EPUB. Temporary path: {staging}; local EPUB is unchanged"))?;
        self.rename(&staging, name).with_context(|| format!("Verified upload remains at {staging}; could not publish {remote}. Nothing was overwritten"))?;
        Ok(Sent {
            path: remote,
            bytes: data.len(),
            already_present: false,
            backup,
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

    fn upload(&self, name: &str, data: &[u8]) -> Result<()> {
        let host = self
            .base
            .host_str()
            .context("Missing reader host")?
            .trim_matches(['[', ']']);
        let addresses = (host, self.websocket_port)
            .to_socket_addrs()
            .context("Cannot resolve reader WebSocket address")?;
        let mut stream = None;
        let deadline = Instant::now() + Duration::from_secs(600);
        let connect_deadline = Instant::now() + Duration::from_secs(5);
        for address in addresses {
            let remaining = connect_deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            if let Ok(socket) = TcpStream::connect_timeout(&address, remaining) {
                stream = Some(socket);
                break;
            }
        }
        let stream = stream.context("Cannot connect to CrossPoint WebSocket port; keep File Transfer open (default port 81)")?;
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        stream.set_write_timeout(Some(Duration::from_secs(30)))?;
        stream.set_nodelay(true)?;
        let mut url = self.base.clone();
        url.set_scheme("ws")
            .map_err(|_| anyhow::anyhow!("Invalid WebSocket URL"))?;
        url.set_port(Some(self.websocket_port))
            .map_err(|_| anyhow::anyhow!("Invalid WebSocket port"))?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(4096))
            .max_frame_size(Some(4096));
        let (mut ws, _) =
            tungstenite::client::client_with_config(url.as_str(), stream, Some(config))
                .context("CrossPoint WebSocket handshake failed")?;
        ws.send(Message::Text(
            format!("START:{name}:{}:{}", data.len(), self.folder).into(),
        ))?;
        ensure!(
            control(&mut ws, deadline)? == "READY",
            "Reader did not acknowledge upload start"
        );
        let mut sent = 0;
        // Match CrossPoint's browser's 4 KiB frames, but wait for its 64 KiB
        // acknowledgements so network buffering cannot outrun the SD writer.
        for window in data.chunks(65536) {
            for chunk in window.chunks(4096) {
                ensure!(Instant::now() < deadline, "Upload timed out");
                ws.send(Message::Binary(chunk.to_vec().into()))?;
            }
            sent += window.len();
            let expected = format!("PROGRESS:{sent}:{}", data.len());
            ensure!(
                control(&mut ws, deadline)? == expected,
                "Reader acknowledged an unexpected byte count"
            );
        }
        ensure!(
            control(&mut ws, deadline)? == "DONE",
            "Reader did not confirm upload completion"
        );
        let _ = ws.close(None);
        Ok(())
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
        let mut url = self.base.join(endpoint)?;
        if let Some(path) = path {
            url.query_pairs_mut().append_pair("path", path);
        }
        let mut easy = Easy::new();
        easy.url(url.as_str())?;
        // Reader traffic must go directly to the supplied device, including on
        // machines with corporate proxy environment variables.
        easy.proxy("")?;
        easy.follow_location(false)?;
        easy.connect_timeout(Duration::from_secs(5))?;
        easy.timeout(Duration::from_secs(
            if endpoint == "/upload" || endpoint == "/download" {
                600
            } else {
                15
            },
        ))?;
        easy.low_speed_limit(128)?;
        easy.low_speed_time(Duration::from_secs(30))?;
        easy.useragent(concat!("xteink/", env!("CARGO_PKG_VERSION")))?;
        if let Some(body) = form {
            let mut headers = List::new();
            headers.append("Content-Type: application/x-www-form-urlencoded")?;
            headers.append("Expect:")?;
            easy.http_headers(headers)?;
            easy.post(true)?;
            easy.post_fields_copy(body.as_bytes())?;
        }
        let mut response = Vec::new();
        let mut too_large = false;
        let result = {
            let mut transfer = easy.transfer();
            transfer.write_function(|bytes| {
                if bytes.len() > limit.saturating_sub(response.len()) {
                    too_large = true;
                    return Ok(0);
                }
                response.extend_from_slice(bytes);
                Ok(bytes.len())
            })?;
            transfer.perform()
        };
        ensure!(
            !too_large,
            "Reader response exceeded the expected size limit"
        );
        result.with_context(|| format!("CrossPoint request failed: {endpoint}"))?;
        let status = easy.response_code()?;
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

fn control(ws: &mut WebSocket<TcpStream>, deadline: Instant) -> Result<String> {
    loop {
        ensure!(
            Instant::now() < deadline,
            "Upload timed out waiting for reader acknowledgement"
        );
        match ws
            .read()
            .context("CrossPoint upload connection interrupted")?
        {
            Message::Text(text) => {
                ensure!(!text.starts_with("ERROR:"), "CrossPoint {}", text);
                return Ok(text.to_string());
            }
            Message::Ping(_) => ws.flush()?,
            Message::Pong(_) => {}
            _ => anyhow::bail!("Unexpected CrossPoint upload response"),
        }
    }
}
