use crossload::crosspoint::Reader;
use std::{
    fs,
    io::{Cursor, Read, Write},
    net::TcpListener,
    path::Path,
    process::Command,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tempfile::TempDir;
use zip::{write::SimpleFileOptions, ZipWriter};

const NAME: &str = "Één boek [test].epub";
fn book() -> (TempDir, Vec<u8>) {
    let dir = tempfile::tempdir().unwrap();
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in [
        ("mimetype", "application/epub+zip"),
        ("META-INF/container.xml", "<container><rootfiles><rootfile full-path='book.opf'/></rootfiles></container>"),
        ("book.opf", "<package><manifest><item id='c' href='chapter.xhtml' media-type='application/xhtml+xml'/></manifest><spine><itemref idref='c'/></spine></package>"),
        ("chapter.xhtml", "<html xmlns='http://www.w3.org/1999/xhtml'><head><title>Test</title></head><body><p>Wi-Fi test</p></body></html>"),
    ] {
        zip.start_file(name, SimpleFileOptions::default()).unwrap(); zip.write_all(data.as_bytes()).unwrap();
    }
    let bytes = zip.finish().unwrap().into_inner();
    fs::write(dir.path().join(NAME), &bytes).unwrap();
    (dir, bytes)
}
struct Step {
    method: &'static str,
    path: String,
    status: u16,
    response: Vec<u8>,
    upload: Option<Vec<u8>>,
}
fn get(path: &str, response: impl AsRef<[u8]>) -> Step {
    Step {
        method: "GET",
        path: path.to_owned(),
        status: 200,
        response: response.as_ref().to_vec(),
        upload: None,
    }
}
fn status() -> Step {
    get("/api/status", br#"{"version":"1.6.0","device":"X4"}"#)
}
fn listing(name: &str, size: usize) -> String {
    serde_json::json!([{"name":name,"size":size,"isDirectory":false}]).to_string()
}
fn upload(bytes: &[u8], status: u16, reply: &str) -> Step {
    Step {
        method: "POST",
        path: "/upload?path=%2F".to_owned(),
        status,
        response: reply.as_bytes().to_vec(),
        upload: Some(bytes.to_vec()),
    }
}
fn remote_path() -> String {
    let mut url = url::Url::parse("http://reader/download").unwrap();
    url.query_pairs_mut()
        .append_pair("path", &format!("/{NAME}"));
    format!("{}?{}", url.path(), url.query().unwrap())
}
struct Mock {
    port: u16,
    handle: JoinHandle<()>,
}
impl Mock {
    fn join(self) -> thread::Result<()> {
        self.handle.join()
    }
}
fn rename() -> Step {
    Step {
        method: "POST",
        path: "/rename".to_owned(),
        status: 200,
        response: b"Renamed successfully".to_vec(),
        upload: None,
    }
}
fn server(steps: Vec<Step>) -> (String, Mock) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let ws_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    ws_listener.set_nonblocking(true).unwrap();
    let port = ws_listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let mut staging = String::new();
        let deadline = Instant::now() + Duration::from_secs(25);
        for step in steps {
            if let Some(expected) = &step.upload {
                let stream = loop {
                    if let Ok((stream, _)) = ws_listener.accept() {
                        break stream;
                    }
                    assert!(Instant::now() < deadline, "Missing WebSocket upload");
                    thread::sleep(Duration::from_millis(5));
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut ws = tungstenite::accept(stream).unwrap();
                let start = ws.read().unwrap().into_text().unwrap();
                let fields: Vec<_> = start.split(':').collect();
                assert_eq!(fields.len(), 4);
                assert_eq!(fields[0], "START");
                assert!(fields[1].starts_with("xteink-"));
                assert!(fields[1].ends_with(".uploading"));
                assert_eq!(fields[2], expected.len().to_string());
                let folder = if step.path.contains("Mick") {
                    "/Mick Herron"
                } else {
                    "/"
                };
                assert_eq!(fields[3], folder);
                staging = format!("{}/{}", folder.trim_end_matches('/'), fields[1]);
                if step.status != 200 {
                    ws.send(tungstenite::Message::Text(
                        "ERROR:Write failed - disk full?".into(),
                    ))
                    .unwrap();
                    continue;
                }
                ws.send(tungstenite::Message::Text("READY".into())).unwrap();
                let mut received = Vec::new();
                while received.len() < expected.len() {
                    let message = ws.read().unwrap();
                    assert!(message.is_binary());
                    let data = message.into_data();
                    assert!(data.len() <= 4096);
                    received.extend_from_slice(&data);
                    if received.len() % 65536 == 0 || received.len() == expected.len() {
                        ws.send(tungstenite::Message::Text(
                            format!("PROGRESS:{}:{}", received.len(), expected.len()).into(),
                        ))
                        .unwrap();
                    }
                }
                assert_eq!(&received, expected);
                ws.send(tungstenite::Message::Text("DONE".into())).unwrap();
                continue;
            }
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "Missing request {}", step.path);
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                header.push(byte[0]);
                assert!(header.len() < 32768);
            }
            let header = String::from_utf8(header).unwrap();
            let expected_path = if !staging.is_empty() && step.path == remote_path() {
                let mut url = url::Url::parse("http://reader/download").unwrap();
                url.query_pairs_mut().append_pair("path", &staging);
                format!("{}?{}", url.path(), url.query().unwrap())
            } else {
                step.path.clone()
            };
            assert!(
                header.starts_with(&format!("{} {} HTTP/1.1\r\n", step.method, expected_path)),
                "{header}"
            );
            let length = header
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            assert!(length < 1024 * 1024);
            let mut body = vec![0; length];
            stream.read_exact(&mut body).unwrap();
            if step.method == "POST" {
                let values: std::collections::HashMap<_, _> =
                    url::form_urlencoded::parse(&body).into_owned().collect();
                if step.path == "/mkdir" {
                    assert_eq!(values["path"], "/");
                    assert_eq!(values["name"], "Mick Herron");
                } else if staging.is_empty() {
                    assert_eq!(values["path"], format!("/{NAME}"));
                    assert!(values["name"].contains(".incomplete-"));
                } else {
                    assert_eq!(values["path"], staging);
                    assert_eq!(values["name"], NAME);
                }
            } else {
                assert!(body.is_empty());
            }
            write!(
                stream,
                "HTTP/1.1 {} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                step.status,
                step.response.len()
            )
            .unwrap();
            stream.write_all(&step.response).unwrap();
        }
    });
    (address, Mock { port, handle })
}
#[test]
fn websocket_upload_verifies_staging_before_publication() {
    let (dir, bytes) = book();
    let (address, server) = server(vec![
        status(),
        get("/api/files?path=%2F", "[]"),
        upload(&bytes, 200, &format!("File uploaded successfully: {NAME}")),
        get(&remote_path(), &bytes),
        rename(),
    ]);
    let sent = Reader::new(&address, "/")
        .unwrap()
        .with_websocket_port(server.port)
        .send(&dir.path().join(NAME))
        .unwrap();
    assert!(!sent.already_present);
    assert_eq!(sent.bytes, bytes.len());
    assert_eq!(sent.path, format!("/{NAME}"));
    assert_eq!(fs::read(dir.path().join(NAME)).unwrap(), bytes);
    server.join().unwrap();
}
#[test]
fn identical_existing_file_is_verified_without_upload() {
    let (dir, bytes) = book();
    let (address, server) = server(vec![
        status(),
        get("/api/files?path=%2F", listing(NAME, bytes.len())),
        get(&remote_path(), &bytes),
    ]);
    assert!(
        Reader::new(&address, "/")
            .unwrap()
            .with_websocket_port(server.port)
            .send(&dir.path().join(NAME))
            .unwrap()
            .already_present
    );
    server.join().unwrap();
}
#[test]
fn collision_with_different_bytes_is_not_overwritten() {
    let (dir, bytes) = book();
    let mut wrong = bytes.clone();
    wrong[0] ^= 1;
    let (address, server) = server(vec![
        status(),
        get("/api/files?path=%2F", listing(NAME, bytes.len())),
        get(&remote_path(), wrong),
    ]);
    let error = Reader::new(&address, "/")
        .unwrap()
        .with_websocket_port(server.port)
        .send(&dir.path().join(NAME))
        .unwrap_err();
    assert!(error
        .to_string()
        .to_lowercase()
        .contains("nothing was overwritten"));
    server.join().unwrap();
}
#[test]
fn corrupt_readback_reports_failure() {
    let (dir, bytes) = book();
    let (address, server) = server(vec![
        status(),
        get("/api/files?path=%2F", "[]"),
        upload(&bytes, 200, &format!("File uploaded successfully: {NAME}")),
        get(&remote_path(), b"truncated"),
    ]);
    let error = Reader::new(&address, "/")
        .unwrap()
        .with_websocket_port(server.port)
        .send(&dir.path().join(NAME))
        .unwrap_err();
    assert!(error.to_string().contains("verification failed"));
    assert_eq!(fs::read(dir.path().join(NAME)).unwrap(), bytes);
    server.join().unwrap();
}
#[test]
fn firmware_upload_failure_is_not_retried() {
    let (dir, bytes) = book();
    let (address, server) = server(vec![
        status(),
        get("/api/files?path=%2F", "[]"),
        upload(&bytes, 400, "File already exists"),
    ]);
    let error = Reader::new(&address, "/")
        .unwrap()
        .with_websocket_port(server.port)
        .send(&dir.path().join(NAME))
        .unwrap_err();
    assert!(format!("{error:#}").contains("Write failed"));
    server.join().unwrap();
}
#[test]
fn missing_folder_rejected_before_upload() {
    let (dir, _) = book();
    let (address, server) = server(vec![status(), get("/api/files?path=%2F", "[]")]);
    let error = Reader::new(&address, "/Books")
        .unwrap()
        .with_websocket_port(server.port)
        .send(&dir.path().join(NAME))
        .unwrap_err();
    assert!(error.to_string().contains("does not exist"));
    server.join().unwrap();
}
#[test]
fn nested_folder_queries_are_encoded() {
    let (dir, bytes) = book();
    let (address, server) = server(vec![
        status(),
        get(
            "/api/files?path=%2F",
            r#"[{"name":"My Books","size":0,"isDirectory":true}]"#,
        ),
        get("/api/files?path=%2FMy+Books", listing(NAME, 0)),
    ]);
    let error = Reader::new(&address, "/My Books/")
        .unwrap()
        .with_websocket_port(server.port)
        .send(&dir.path().join(NAME))
        .unwrap_err();
    assert!(error
        .to_string()
        .to_lowercase()
        .contains("nothing was overwritten"));
    assert!(!bytes.is_empty());
    server.join().unwrap();
}
#[test]
fn invalid_input_and_unsafe_destinations_fail_before_network() {
    for address in [
        "",
        "file:///tmp",
        "https://reader",
        "http://user:secret@reader",
        "http://reader/upload",
        "http://reader?x=1",
        "http://reader/#x",
    ] {
        assert!(Reader::new(address, "/").is_err(), "{address}");
    }
    for folder in [
        "Books",
        "/..",
        "/.crosspoint",
        "/Books/../Other",
        "/Books//Other",
        "/fonts\\bad",
        "/XTCache",
    ] {
        assert!(Reader::new("127.0.0.1:9", folder).is_err(), "{folder}");
    }
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("bad.epub"), b"not an EPUB").unwrap();
    assert!(Reader::new("127.0.0.1:9", "/")
        .unwrap()
        .send(&dir.path().join("bad.epub"))
        .unwrap_err()
        .to_string()
        .contains("validation"));
    assert!(Reader::new("127.0.0.1:9", "/")
        .unwrap()
        .send(Path::new("../bad\n.epub"))
        .is_err());
}
#[test]
fn wrong_server_response_is_rejected() {
    let (dir, _) = book();
    let (address, server) = server(vec![get("/api/status", "<html>login</html>")]);
    assert!(Reader::new(&address, "/")
        .unwrap()
        .with_websocket_port(server.port)
        .send(&dir.path().join(NAME))
        .is_err());
    server.join().unwrap();
}
#[test]
fn send_cli_verifies_existing_file_and_bypasses_proxy() {
    let (dir, bytes) = book();
    let (address, server) = server(vec![
        status(),
        get("/api/files?path=%2F", listing(NAME, bytes.len())),
        get(&remote_path(), &bytes),
    ]);
    let result = Command::new(env!("CARGO_BIN_EXE_xteink"))
        .arg("send")
        .arg(dir.path().join(NAME))
        .args(["--to", &address, "--flat", "--no-optimize"])
        .env("http_proxy", "http://127.0.0.1:9")
        .env("ALL_PROXY", "http://127.0.0.1:9")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("Already on reader"));
    assert!(String::from_utf8_lossy(&result.stdout).contains("SHA-256 verified"));
    server.join().unwrap();
}
#[test]
fn import_folder_requires_send_to() {
    let result = Command::new(env!("CARGO_BIN_EXE_xteink"))
        .args([
            "import",
            "book.acsm",
            "--output",
            "books",
            "--folder",
            "/Books",
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("--send-to"));
}

#[test]
fn repair_preserves_verified_prefix_then_sends_fresh_copy() {
    let (dir, bytes) = book();
    let partial = &bytes[..100];
    let (address, server) = server(vec![
        status(),
        get("/api/files?path=%2F", listing(NAME, partial.len())),
        get(&remote_path(), partial),
        rename(),
        upload(&bytes, 200, ""),
        get(&remote_path(), &bytes),
        rename(),
    ]);
    let sent = Reader::new(&address, "/")
        .unwrap()
        .with_websocket_port(server.port)
        .repair_incomplete(true)
        .send(&dir.path().join(NAME))
        .unwrap();
    assert!(!sent.already_present);
    assert!(sent.backup.unwrap().contains(".incomplete-100"));
    server.join().unwrap();
}
#[test]
fn repair_refuses_unrelated_shorter_file() {
    let (dir, _) = book();
    let (address, server) = server(vec![
        status(),
        get("/api/files?path=%2F", listing(NAME, 4)),
        get(&remote_path(), b"nope"),
    ]);
    let error = Reader::new(&address, "/")
        .unwrap()
        .repair_incomplete(true)
        .send(&dir.path().join(NAME))
        .unwrap_err();
    assert!(error.to_string().contains("repair refused"));
    server.join().unwrap();
}
#[test]
fn websocket_handles_multiple_acknowledgement_windows() {
    let (dir, _) = book();
    let path = dir.path().join(NAME);
    let mut zip = ZipWriter::new_append(
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap(),
    )
    .unwrap();
    zip.start_file(
        "large.bin",
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )
    .unwrap();
    zip.write_all(&vec![0x42; 150000]).unwrap();
    zip.finish().unwrap();
    let bytes = fs::read(&path).unwrap();
    let (address, server) = server(vec![
        status(),
        get("/api/files?path=%2F", "[]"),
        upload(&bytes, 200, ""),
        get(&remote_path(), &bytes),
        rename(),
    ]);
    let sent = Reader::new(&address, "/")
        .unwrap()
        .with_websocket_port(server.port)
        .send(&path)
        .unwrap();
    assert_eq!(sent.bytes, bytes.len());
    server.join().unwrap();
}

#[test]
fn author_folder_is_created_and_existing_case_is_reused() {
    let (address, mock) = server(vec![
        status(),
        get("/api/files?path=%2F", "[]"),
        Step {
            method: "POST",
            path: "/mkdir".to_owned(),
            status: 200,
            response: b"Folder created: Mick Herron".to_vec(),
            upload: None,
        },
    ]);
    assert!(Reader::new(&address, "/")
        .unwrap()
        .for_author("Mick Herron")
        .is_ok());
    mock.join().unwrap();
    let (address, mock) = server(vec![
        status(),
        get(
            "/api/files?path=%2F",
            r#"[{"name":"MICK HERRON","size":0,"isDirectory":true}]"#,
        ),
    ]);
    assert!(Reader::new(&address, "/")
        .unwrap()
        .for_author("Mick Herron")
        .is_ok());
    mock.join().unwrap();
}

#[test]
fn author_folder_collision_is_rejected() {
    let (address, mock) = server(vec![
        status(),
        get("/api/files?path=%2F", listing("Mick Herron", 3)),
    ]);
    assert!(Reader::new(&address, "/")
        .unwrap()
        .for_author("Mick Herron")
        .is_err());
    mock.join().unwrap();
}

#[test]
fn author_folder_transfer_stages_verifies_and_publishes_inside_folder() {
    let (dir, bytes) = book();
    let mut transfer = upload(&bytes, 200, "");
    transfer.path = "/upload?path=%2FMick+Herron".to_owned();
    let (address, mock) = server(vec![
        status(),
        get("/api/files?path=%2F", "[]"),
        Step {
            method: "POST",
            path: "/mkdir".to_owned(),
            status: 200,
            response: b"Folder created: Mick Herron".to_vec(),
            upload: None,
        },
        status(),
        get(
            "/api/files?path=%2F",
            r#"[{"name":"Mick Herron","size":0,"isDirectory":true}]"#,
        ),
        get("/api/files?path=%2FMick+Herron", "[]"),
        transfer,
        get(&remote_path(), &bytes),
        rename(),
    ]);
    let sent = Reader::new(&address, "/")
        .unwrap()
        .with_websocket_port(mock.port)
        .for_author("Mick Herron")
        .unwrap()
        .send(&dir.path().join(NAME))
        .unwrap();
    assert_eq!(sent.path, format!("/Mick Herron/{NAME}"));
    mock.join().unwrap();
}
