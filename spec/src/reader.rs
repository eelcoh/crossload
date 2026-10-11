//! A CrossPoint reader in File Transfer mode, simulated: its HTTP endpoints (status, listings,
//! downloads, folders, renames) on a `SimHttp`, and its upload protocol on a `SimWebSocket`.
//! It keeps a card of files, so what the app uploads can be listed, downloaded and checked.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};

use ambit_caps_net::{http, HttpError, Response, WsMessage};
use ambit_caps_sim::{SimHttp, SimWebSocket};

/// The address the simulated reader answers on, inside the wiring's local-network scope.
pub const IP: &str = "192.168.1.50";

#[derive(Clone)]
pub struct FakeReader {
    state: Arc<Mutex<State>>,
}

struct State {
    device: String,
    files: BTreeMap<String, Vec<u8>>,
    dirs: BTreeSet<String>,
    /// The reader refuses uploads with this error.
    refuse: Option<String>,
    /// Downloads of uploads in progress come back truncated.
    corrupt: bool,
    /// The status endpoint answers this instead.
    status: Option<String>,
    /// Requests, as `METHOD /path?query`, in order.
    log: Vec<String>,
    /// Upload frames larger than this were seen.
    largest_frame: usize,
}

fn parent(path: &str) -> String {
    match path.rsplit_once('/') {
        Some(("", _)) | None => "/".into(),
        Some((parent, _)) => parent.into(),
    }
}

fn join(folder: &str, name: &str) -> String {
    format!("{}/{name}", folder.trim_end_matches('/'))
}

fn reply(status: u16, body: impl Into<Vec<u8>>) -> Option<Result<Response, HttpError>> {
    Some(Ok(http::Response::builder()
        .status(status)
        .body(body.into())
        .unwrap()))
}

impl Default for FakeReader {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeReader {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                device: "X4".into(),
                files: BTreeMap::new(),
                dirs: BTreeSet::from(["/".to_string()]),
                refuse: None,
                corrupt: false,
                status: None,
                log: vec![],
                largest_frame: 0,
            })),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A file on the reader's card, and the folders above it.
    pub fn file(&self, path: &str, bytes: impl Into<Vec<u8>>) -> &Self {
        let mut state = self.state();
        let mut folder = parent(path);
        while folder != "/" {
            state.dirs.insert(folder.clone());
            folder = parent(&folder);
        }
        state.files.insert(path.into(), bytes.into());
        self
    }

    pub fn dir(&self, path: &str) -> &Self {
        self.state().dirs.insert(path.into());
        self
    }

    pub fn device(&self, name: &str) -> &Self {
        self.state().device = name.into();
        self
    }

    pub fn refuse_uploads(&self, error: &str) -> &Self {
        self.state().refuse = Some(error.into());
        self
    }

    pub fn corrupt_readback(&self) -> &Self {
        self.state().corrupt = true;
        self
    }

    pub fn status_body(&self, body: &str) -> &Self {
        self.state().status = Some(body.into());
        self
    }

    pub fn files(&self) -> BTreeMap<String, Vec<u8>> {
        self.state().files.clone()
    }

    pub fn get(&self, path: &str) -> Option<Vec<u8>> {
        self.state().files.get(path).cloned()
    }

    pub fn log(&self) -> Vec<String> {
        self.state().log.clone()
    }

    pub fn largest_frame(&self) -> usize {
        self.state().largest_frame
    }

    /// Answer on `http` and `upload`, at [`IP`].
    pub fn serve(&self, http: &SimHttp, upload: &SimWebSocket) {
        let reader = self.clone();
        http.handle(move |request| {
            if request.uri().host() != Some(IP) {
                return None;
            }
            reader.http(request)
        });
        let reader = self.clone();
        upload.serve(move |url| {
            if !url.starts_with(&format!("ws://{IP}:81")) {
                return None;
            }
            let reader = reader.clone();
            let mut upload: Option<(String, usize, Vec<u8>)> = None;
            Some(Box::new(move |message| reader.upload(&mut upload, message)))
        });
    }

    fn http(&self, request: &ambit_caps_net::Request) -> Option<Result<Response, HttpError>> {
        let uri = request.uri();
        let query: BTreeMap<String, String> =
            url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes())
                .into_owned()
                .collect();
        let form: BTreeMap<String, String> = url::form_urlencoded::parse(request.body())
            .into_owned()
            .collect();
        let mut state = self.state();
        state.log.push(format!(
            "{} {}",
            request.method(),
            uri.path_and_query().map_or("", |p| p.as_str())
        ));
        match (request.method().as_str(), uri.path()) {
            ("GET", "/api/status") => {
                let body = state.status.clone().unwrap_or_else(|| {
                    format!(r#"{{"version":"1.6.0","device":"{}"}}"#, state.device)
                });
                reply(200, body)
            }
            ("GET", "/api/files") => {
                let folder = query.get("path").cloned().unwrap_or_else(|| "/".into());
                // CrossPoint answers an empty listing for a folder that is not there.
                let mut entries = vec![];
                for dir in &state.dirs {
                    if dir != "/" && parent(dir) == folder {
                        let name = dir.rsplit('/').next().unwrap();
                        entries.push(
                            serde_json::json!({"name": name, "size": 0, "isDirectory": true}),
                        );
                    }
                }
                for (path, bytes) in &state.files {
                    if parent(path) == folder {
                        let name = path.rsplit('/').next().unwrap();
                        entries.push(serde_json::json!({"name": name, "size": bytes.len(), "isDirectory": false}));
                    }
                }
                reply(200, serde_json::Value::Array(entries).to_string())
            }
            ("GET", "/download") => {
                let path = query.get("path")?;
                match state.files.get(path) {
                    Some(_) if state.corrupt && path.ends_with(".uploading") => {
                        reply(200, &b"truncated"[..])
                    }
                    Some(bytes) => reply(200, bytes.clone()),
                    None => reply(404, "Not found"),
                }
            }
            ("POST", "/mkdir") => {
                let path = join(form.get("path")?, form.get("name")?);
                state.dirs.insert(path.clone());
                reply(200, format!("Folder created: {}", form.get("name")?))
            }
            ("POST", "/rename") => {
                let from = form.get("path")?.clone();
                let to = join(&parent(&from), form.get("name")?);
                if state.files.contains_key(&to) {
                    return reply(400, "File already exists");
                }
                match state.files.remove(&from) {
                    Some(bytes) => {
                        state.files.insert(to, bytes);
                        reply(200, "Renamed successfully")
                    }
                    None => reply(404, "Not found"),
                }
            }
            _ => reply(404, "Not found"),
        }
    }

    /// The upload protocol, one message at a time: `START:name:size:folder`, `READY`, binary
    /// frames, `PROGRESS:n:size` every 64 KiB and at the end, then `DONE`.
    fn upload(
        &self,
        upload: &mut Option<(String, usize, Vec<u8>)>,
        message: WsMessage,
    ) -> Vec<WsMessage> {
        let mut state = self.state();
        match message {
            WsMessage::Text(text) => {
                let fields: Vec<&str> = text.splitn(4, ':').collect();
                assert_eq!(fields.len(), 4, "{text}");
                assert_eq!(fields[0], "START");
                assert!(fields[1].starts_with("xteink-") && fields[1].ends_with(".uploading"));
                if let Some(error) = &state.refuse {
                    return vec![WsMessage::Text(format!("ERROR:{error}"))];
                }
                *upload = Some((
                    join(fields[3], fields[1]),
                    fields[2].parse().unwrap(),
                    vec![],
                ));
                vec![WsMessage::Text("READY".into())]
            }
            WsMessage::Binary(data) => {
                state.largest_frame = state.largest_frame.max(data.len());
                let (path, size, received) = upload.as_mut().expect("START first");
                received.extend_from_slice(&data);
                let mut replies = vec![];
                if received.len() % 65536 == 0 || received.len() == *size {
                    replies.push(WsMessage::Text(format!(
                        "PROGRESS:{}:{size}",
                        received.len()
                    )));
                }
                if received.len() == *size {
                    state.files.insert(path.clone(), received.clone());
                    replies.push(WsMessage::Text("DONE".into()));
                }
                replies
            }
        }
    }
}
