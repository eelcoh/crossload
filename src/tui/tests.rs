use super::*;
use crossterm::event::{Event, KeyCode, KeyEvent};
use std::sync::{Arc, Mutex};
fn options() -> Options {
    Options {
        show_previews: false,
        device: None,
        browse: "/books".into(),
        output: "/output".into(),
        state: None,
        send_to: None,
        copy_to: None,
        folder: "/".into(),
        optimize: true,
        organized: true,
        serial: None,
    }
}
fn key(code: KeyCode) -> Message {
    Message::Input(Event::Key(KeyEvent::from(code)))
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
fn catalog_stale_updates_actions_and_quit_are_safe() {
    let mut model = Model::new(options());
    model.update(Message::Refresh);
    let id = model.loading.unwrap();
    let mut snapshot = crate::books::Snapshot::default();
    snapshot.acsm.push("/book.acsm".into());
    model.update(Message::Catalog(id + 1, snapshot.clone()));
    assert!(model.entries.is_empty());
    model.update(Message::Catalog(id, snapshot));
    assert_eq!(model.entries.len(), 1);
    model.update(key(KeyCode::Enter));
    assert!(model.action.is_some());
    assert!(model.update(key(KeyCode::Char('2'))).is_empty());
    assert!(model.busy.is_none());
    let effects = model.update(key(KeyCode::Char('1')));
    assert!(matches!(effects.as_slice(), [Effect::Work { .. }]));
    let job = model.busy.unwrap();
    assert!(model.update(key(KeyCode::Enter)).is_empty());
    model.update(key(KeyCode::Char('/')));
    model.update(key(KeyCode::Char('b')));
    assert_eq!(model.query, "b");
    model.update(key(KeyCode::Esc));
    assert!(model.update(key(KeyCode::Char('q'))).is_empty());
    assert!(model
        .update(Message::Finished(job, Ok("done".into())))
        .is_empty());
    assert!(matches!(
        model
            .update(Message::CatalogFinished(id, Ok(())))
            .as_slice(),
        [Effect::Quit]
    ));
}
#[test]
fn view_and_navigation_handle_sizes_and_empty_search() {
    let mut model = Model::new(options());
    model.entries = (0..25)
        .map(|i| Entry {
            title: format!("Book {i}\x1b"),
            author: "Writer".into(),
            kind: "ACSM",
            source: Source::Local(format!("/{i}.acsm").into()),
        })
        .collect();
    model.update(key(KeyCode::PageDown));
    assert_eq!(model.selected, 10);
    model.update(key(KeyCode::End));
    assert_eq!(model.selected, 24);
    model.update(key(KeyCode::PageDown));
    assert_eq!(model.selected, 24);
    for (w, h) in [(1, 1), (25, 10), (80, 24), (120, 35)] {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        terminal.draw(|frame| view::draw(&model, frame)).unwrap();
        assert!(!format!("{:?}", terminal.backend().buffer()).contains("\\u{1b}"));
    }
    model.query = "absent".into();
    model.update(key(KeyCode::End));
    assert_eq!(model.selected, 0);
}
