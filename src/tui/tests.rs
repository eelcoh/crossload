use super::*;
use crossterm::event::{Event, KeyCode, KeyEvent};
use std::sync::{Arc, Mutex};
fn options() -> Options {
    Options {
        show_previews: false,
        device: Some("/kobo".into()),
        browse: "/books".into(),
        output: "/output".into(),
        state: None,
        send_to: Some("reader.local".into()),
        copy_to: None,
        folder: "/".into(),
        optimize: true,
        organized: true,
        serial: None,
    }
}
fn entry(id: &str) -> Entry {
    Entry {
        title: id.into(),
        author: "Writer".into(),
        kind: "Kobo",
        source: Source::Kobo(id.into()),
        preview: false,
    }
}
fn key(code: KeyCode) -> Message {
    Message::Input(Event::Key(KeyEvent::from(code)))
}
#[test]
fn rejects_stale_loads_and_preserves_selection_on_refresh() {
    let mut model = Model::new(options());
    model.entries = vec![entry("a"), entry("b")];
    model.selected = 1;
    let _ = model.update(Message::Refresh);
    let old = model.loading.unwrap();
    let _ = model.update(Message::Refresh);
    let current = model.loading.unwrap();
    let _ = model.update(Message::Loaded(old, Ok(vec![])));
    assert_eq!(model.entries.len(), 2);
    let _ = model.update(Message::Loaded(current, Ok(vec![entry("b"), entry("a")])));
    assert_eq!(model.selected, 0);
}
#[test]
fn duplicate_jobs_previews_and_stale_progress_are_rejected() {
    let mut model = Model::new(options());
    model.entries = vec![entry("book")];
    let effects = model.update(key(KeyCode::Enter));
    assert!(
        matches!(&effects[0], Effect::Work { entry, options, .. } if entry.title == "book" && options.send_to.as_deref() == Some("reader.local"))
    );
    let id = model.busy.unwrap();
    assert!(model.update(key(KeyCode::Enter)).is_empty());
    let _ = model.update(Message::Progress(id + 1, "stale".into()));
    assert_ne!(model.status, "stale");
    let _ = model.update(Message::Finished(id + 1, Ok("stale".into())));
    assert_eq!(model.busy, Some(id));
    let _ = model.update(Message::Finished(id, Ok("done".into())));
    model.entries[0].preview = true;
    assert!(model.update(key(KeyCode::Enter)).is_empty());
    assert!(model.busy.is_none());
}
#[test]
fn quit_waits_for_work_and_search_stays_responsive() {
    let mut model = Model::new(options());
    model.entries = vec![entry("book")];
    let _ = model.update(key(KeyCode::Enter));
    let id = model.busy.unwrap();
    let _ = model.update(key(KeyCode::Char('/')));
    let _ = model.update(key(KeyCode::Char('b')));
    assert_eq!(model.query, "b");
    let _ = model.update(key(KeyCode::Esc));
    assert!(model.update(key(KeyCode::Char('q'))).is_empty());
    assert!(model.pending_quit);
    assert!(matches!(
        model
            .update(Message::Finished(id, Ok("done".into())))
            .as_slice(),
        [Effect::Quit]
    ));
}
#[test]
fn view_handles_small_sizes_and_sanitizes_metadata() {
    let mut model = Model::new(options());
    model.entries = vec![entry("bad\x1b[31m\nname")];
    for (w, h) in [(1, 1), (20, 8), (80, 24), (120, 35)] {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        terminal.draw(|frame| view::draw(&model, frame)).unwrap();
        assert!(!format!("{:?}", terminal.backend().buffer()).contains("\\u{1b}"));
    }
}

struct Proof {
    log: Arc<Mutex<Vec<String>>>,
    release: Option<std::sync::mpsc::Sender<()>>,
    pending: bool,
}
impl Application for Proof {
    type Message = Message;
    type Flags = (Arc<Mutex<Vec<String>>>, u8);
    fn new((log, mode): Self::Flags) -> (Self, Command<Message>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let cmd = Command::stream(work_stream(1, move |progress| {
            progress("started");
            rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
            match mode {
                1 => anyhow::bail!("synthetic failure"),
                2 => panic!("synthetic worker panic"),
                _ => Ok("completed".into()),
            }
        }));
        (
            Self {
                log,
                release: Some(tx),
                pending: false,
            },
            cmd,
        )
    }
    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::Progress(_, text) => {
                self.log.lock().unwrap().push(text);
                self.pending = true;
                self.release.take().unwrap().send(()).unwrap();
            }
            Message::Finished(_, result) => {
                assert!(self.pending);
                self.log
                    .lock()
                    .unwrap()
                    .push(result.unwrap_or_else(|e| format!("error: {e}")));
                return Command::effect(tears::Action::Quit);
            }
            _ => {}
        }
        Command::none()
    }
    fn view(&self, frame: &mut ratatui::Frame<'_>) {
        frame.render_widget(ratatui::widgets::Paragraph::new("Proof"), frame.area());
    }
    fn subscriptions(&self) -> Vec<Subscription<Message>> {
        vec![]
    }
}
#[test]
fn tears_runtime_proof_blocking_progress_completion_error_and_panic() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for mode in 0..3 {
        let log = Arc::new(Mutex::new(vec![]));
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        runtime.block_on(async {
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                tears::Runtime::<Proof>::new((log.clone(), mode), 30).run(&mut terminal),
            )
            .await
            .unwrap()
            .unwrap();
        });
        let log = log.lock().unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0], "started");
        if mode == 0 {
            assert_eq!(log[1], "completed");
        } else {
            assert!(log[1].contains("error:"));
        }
    }
}

#[test]
fn reader_status_rejects_stale_results_and_keeps_search_responsive() {
    let mut model = Model::new(options());
    model.update(Message::Refresh);
    let load = model.loading.unwrap();
    let effects = model.update(Message::Loaded(load, Ok(vec![entry("book")])));
    assert!(matches!(effects.as_slice(), [Effect::Check { .. }]));
    let check = model.checking.unwrap();
    model.update(key(KeyCode::Char('/')));
    model.update(key(KeyCode::Char('b')));
    assert_eq!(model.filtered().len(), 1);
    model.update(key(KeyCode::Esc));
    assert!(model.update(key(KeyCode::Enter)).is_empty());
    model.update(Message::Presence(
        check + 1,
        Ok(vec![(Source::Kobo("book".into()), "Present".into())]),
    ));
    assert!(model.presence.is_empty());
    model.update(Message::Presence(check, Err("offline".into())));
    assert!(model.presence.is_empty());
    assert!(model.status.contains("unknown"));
    model.update(Message::Refresh);
    let load = model.loading.unwrap();
    model.update(Message::Loaded(load, Ok(vec![entry("book")])));
    let check = model.checking.unwrap();
    assert!(model.update(key(KeyCode::Char('q'))).is_empty());
    assert!(matches!(
        model
            .update(Message::Presence(check, Ok(vec![])))
            .as_slice(),
        [Effect::Quit]
    ));
}

#[test]
fn reader_inventory_checks_local_contents_without_publishing() {
    use std::io::Write;
    let temp = tempfile::tempdir().unwrap();
    let card = temp.path().join("card");
    std::fs::create_dir(&card).unwrap();
    let source = temp.path().join("book.epub");
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, data) in [
        ("mimetype", "application/epub+zip"),
        ("META-INF/container.xml", "<container><rootfiles><rootfile full-path='book.opf'/></rootfiles></container>"),
        ("book.opf", "<package><metadata><title>Book</title></metadata><manifest><item id='c' href='c.xhtml' media-type='application/xhtml+xml'/></manifest><spine><itemref idref='c'/></spine></package>"),
        ("c.xhtml", "<html><head><title>Book</title></head><body>Text</body></html>"),
    ] { zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap(); zip.write_all(data.as_bytes()).unwrap(); }
    std::fs::write(&source, zip.finish().unwrap().into_inner()).unwrap();
    let mut opts = options();
    opts.device = None;
    opts.send_to = None;
    opts.copy_to = Some(card.clone());
    opts.output = temp.path().join("unpublished");
    let mut book = entry("book");
    book.kind = "EPUB";
    book.source = Source::Local(source.clone());
    assert_eq!(
        backend::presence(&opts, vec![book.clone()]).unwrap()[0].1,
        "Missing"
    );
    std::fs::copy(&source, card.join("renamed.epub")).unwrap();
    assert_eq!(
        backend::presence(&opts, vec![book.clone()]).unwrap()[0].1,
        "Present"
    );
    std::fs::write(&source, b"broken").unwrap();
    assert_eq!(
        backend::presence(&opts, vec![book]).unwrap()[0].1,
        "Unknown"
    );
    assert!(!opts.output.exists());
}
