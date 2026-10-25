//! The command line runs locked on Linux, and still does its job: copy, send and a catalogue
//! download, each run as the program. Their folders are outside the system's temporary folder,
//! which the lock allows for the activation's cookie jar and a Kobo's snapshot, so what they reach
//! they reach through what the command named. ACSM fulfilment and the Kobo's database are tested
//! locked beside their other tests (`adobe.rs`, `kobo.rs`); the terminal interface in
//! `scripts/test-tui.py`.
#![cfg(target_os = "linux")]

use std::{
    fs,
    io::{BufRead, BufReader, Cursor, Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use sha2::{Digest, Sha256};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

/// A place for one test, outside the system's temporary folder.
fn place() -> tempfile::TempDir {
    tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap()
}

/// The program, with a home of its own in `place`.
fn crossload(place: &Path) -> Command {
    at(Command::new(env!("CARGO_BIN_EXE_crossload")), place)
}

/// `command` run in `place`, with a home of its own there.
fn at(mut command: Command, place: &Path) -> Command {
    let home = place.join("home");
    command
        .current_dir(place)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_DATA_HOME", home.join("data"));
    command
}

fn succeeds(command: &mut Command) -> String {
    let output = command.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "{stderr}");
    assert!(!stderr.contains("lockdown: not enforced"), "{stderr}");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn epub(title: &str, author: &str) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(vec![]));
    for (name, value) in [
        ("mimetype", "application/epub+zip".to_owned()),
        ("META-INF/container.xml", "<container><rootfiles><rootfile full-path='book.opf'/></rootfiles></container>".into()),
        ("book.opf", format!("<package xmlns:dc='http://purl.org/dc/elements/1.1/'><metadata><dc:title>{title}</dc:title><dc:creator>{author}</dc:creator></metadata><manifest><item id='c' href='chapter.xhtml' media-type='application/xhtml+xml'/></manifest><spine><itemref idref='c'/></spine></package>")),
        ("chapter.xhtml", format!("<html><head/><body><p>{title}</p></body></html>")),
    ] {
        zip.start_file(name, SimpleFileOptions::default().compression_method(CompressionMethod::Stored)).unwrap();
        zip.write_all(value.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// A web server on a free local port: `answer` gives the body for a request's path, `None`
/// leaves the request waiting.
fn serve(answer: impl Fn(&str) -> Option<Vec<u8>> + Send + 'static) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut line = String::new();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            reader.read_line(&mut line).unwrap();
            let path = line.split(' ').nth(1).unwrap_or("/").to_owned();
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap() <= 2 {
                    break;
                }
            }
            match answer(&path) {
                Some(body) => {
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body);
                }
                None => std::thread::sleep(Duration::from_secs(60)),
            }
        }
    });
    port
}

#[test]
fn copy_to_a_card_runs_locked() {
    let place = place();
    let (books, card) = (place.path().join("books"), place.path().join("card"));
    fs::create_dir_all(&books).unwrap();
    fs::create_dir_all(&card).unwrap();
    let book = epub("Dune", "Frank Herbert");
    fs::write(books.join("dune.epub"), &book).unwrap();
    succeeds(
        crossload(place.path())
            .args(["--no-optimize", "copy"])
            .arg(books.join("dune.epub"))
            .arg("--to")
            .arg(&card),
    );
    assert_eq!(
        fs::read(card.join("Frank Herbert/Dune.epub")).unwrap(),
        book
    );
}

#[test]
fn a_catalogue_download_runs_locked_into_an_output_folder_made_for_it() {
    let place = place();
    let book = epub("Persuasion", "Jane Austen");
    let served = book.clone();
    let port = serve(move |path| (path == "/book.epub").then(|| served.clone()));
    let output = place.path().join("not made yet/imports");
    succeeds(
        crossload(place.path())
            .args(["catalog", "get"])
            .arg(format!("http://localhost:{port}/book.epub"))
            .arg("--output")
            .arg(&output),
    );
    let made: Vec<PathBuf> = walk(&output);
    assert_eq!(made.len(), 1, "{made:?}");
    assert_eq!(fs::read(&made[0]).unwrap(), book);
}

/// Sends to a reader on this machine: its ports (80 and 81) need a network of the test's own,
/// so the test is skipped where unprivileged user namespaces aren't allowed.
#[test]
fn send_to_a_reader_runs_locked() {
    let ok = Command::new("unshare")
        .args(["-rn", "true"])
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        eprintln!("skipped: no unprivileged network namespace here (unshare -rn)");
        return;
    }
    let place = place();
    let reader = place.path().join("reader");
    fs::create_dir_all(&reader).unwrap();
    let book = epub("Middlemarch", "George Eliot");
    fs::write(place.path().join("middlemarch.epub"), &book).unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake_crosspoint.py");
    let program = env!("CARGO_BIN_EXE_crossload");
    let shell = format!(
        "ip link set lo up || exit 1
         python3 '{script}' '{reader}' >/dev/null 2>&1 & reader=$!
         for i in $(seq 100); do [ -e '{reader}/ready' ] && break; sleep 0.1; done
         '{program}' --no-optimize send middlemarch.epub --to localhost; sent=$?
         kill $reader; exit $sent",
        script = script.display(),
        reader = reader.display(),
    );
    succeeds(at(Command::new("unshare"), place.path()).args(["-rn", "sh", "-c", &shell]));
    let files: serde_json::Value =
        serde_json::from_slice(&fs::read(reader.join("files.json")).unwrap()).unwrap();
    assert_eq!(
        files["/George Eliot/Middlemarch.epub"],
        format!("{:x}", Sha256::digest(&book)),
        "{files}"
    );
}

/// The program is locked while it runs: seccomp filters it (2 in `/proc/<pid>/status`), and it
/// can't gain privileges. Seen while it waits on a catalogue that doesn't answer. What was
/// started is its supervisor, which stays outside the lock to start it again; the program is
/// the supervisor's one child.
#[test]
fn the_command_line_is_locked_while_it_runs() {
    let place = place();
    let port = serve(|_| None);
    let mut child = crossload(place.path())
        .args(["catalog", "browse"])
        .arg(format!("http://localhost:{port}/feed"))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let children = format!("/proc/{0}/task/{0}/children", child.id());
    let deadline = Instant::now() + Duration::from_secs(10);
    let (mut program, mut seen) = (None, String::new());
    while Instant::now() < deadline {
        program = fs::read_to_string(&children)
            .unwrap_or_default()
            .split_whitespace()
            .next()
            .map(str::to_owned);
        if let Some(program) = &program {
            seen = fs::read_to_string(format!("/proc/{program}/status")).unwrap_or_default();
            if seen.contains("Seccomp:\t2") {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let supervisor = fs::read_to_string(format!("/proc/{}/status", child.id())).unwrap();
    // The program first: the supervisor, killed, would leave it running.
    if let Some(program) = &program {
        Command::new("kill")
            .args(["-KILL", program])
            .status()
            .unwrap();
    }
    child.kill().unwrap();
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    child.wait().unwrap();
    assert!(seen.contains("Seccomp:\t2"), "{seen}\n{stderr}");
    assert!(seen.contains("NoNewPrivs:\t1"), "{seen}");
    assert!(supervisor.contains("Seccomp:\t0"), "{supervisor}");
}

fn walk(path: &Path) -> Vec<PathBuf> {
    let mut found = vec![];
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(walk(&path));
        } else {
            found.push(path);
        }
    }
    found
}
