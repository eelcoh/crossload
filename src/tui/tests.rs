use super::*;
use crate::books::Place;
use crossterm::event::{Event, KeyCode, KeyEvent};
use model::Destination;
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
            progress(backend::Report::Text("started"));
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
    assert!(!model.action.is_empty());
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
        .map(|i| {
            Entry::new(
                format!("Book {i}\x1b"),
                "Writer".into(),
                "ACSM",
                Source::Local(format!("/{i}.acsm").into()),
            )
        })
        .collect();
    model.update(key(KeyCode::PageDown));
    assert_eq!(model.selected, 10);
    model.update(key(KeyCode::End));
    assert_eq!(model.selected, 24);
    model.update(key(KeyCode::PageDown));
    assert_eq!(model.selected, 24);
    // Every size renders with and without the copy dialog over the list.
    for open in [false, true] {
        model.action = if open {
            vec![model.entries[0].clone()]
        } else {
            vec![]
        };
        for (w, h) in [(1, 1), (25, 10), (26, 11), (52, 20), (80, 24), (120, 35)] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            terminal.draw(|frame| view::draw(&model, frame)).unwrap();
            assert!(!format!("{:?}", terminal.backend().buffer()).contains("\\u{1b}"));
        }
    }
    model.action.clear();
    model.query = "absent".into();
    model.update(key(KeyCode::End));
    assert_eq!(model.selected, 0);
}

#[test]
fn search_navigation_and_refresh_preserve_visible_selection() {
    let mut model = Model::new(options());
    model.update(Message::Refresh);
    let id = model.loading.unwrap();
    let mut snapshot = crate::books::Snapshot {
        acsm: vec!["/a.acsm".into(), "/b.acsm".into(), "/c.acsm".into()],
        ..Default::default()
    };
    model.update(Message::Catalog(id, snapshot.clone()));
    model.update(Message::CatalogFinished(id, Ok(())));
    model.update(key(KeyCode::Char('/')));
    model.update(key(KeyCode::Char('a')));
    model.update(key(KeyCode::Down));
    assert_eq!(model.selected, 1);
    assert!(model.search);
    assert_eq!(model.query, "a");
    model.update(key(KeyCode::Up));
    assert_eq!(model.selected, 0);
    model.update(key(KeyCode::End));
    assert_eq!(model.selected, 2);
    model.update(key(KeyCode::PageUp));
    assert_eq!(model.selected, 0);
    model.update(key(KeyCode::Down));
    model.update(key(KeyCode::Esc));
    model.update(Message::Refresh);
    let refresh = model.loading.unwrap();
    assert_eq!(model.entries.len(), 3);
    assert_eq!(model.selected, 1);
    model.update(key(KeyCode::Enter));
    assert!(model.update(key(KeyCode::Char('1'))).is_empty());
    assert!(model.busy.is_none());
    model.update(key(KeyCode::Esc));
    model.update(Message::Catalog(refresh, crate::books::Snapshot::default()));
    assert_eq!(model.entries.len(), 3);
    snapshot.acsm.remove(0);
    model.update(Message::Catalog(refresh, snapshot));
    assert_eq!(model.selected, 1);
    model.update(Message::CatalogFinished(refresh, Ok(())));
    assert_eq!(model.entries.len(), 2);
    assert_eq!(model.selected, 0);
    assert_eq!(model.query, "a");
    assert_eq!(model.entries[0].source, Source::Local("/b.acsm".into()));
    model.update(Message::Refresh);
    let refresh = model.loading.unwrap();
    model.update(Message::CatalogFinished(refresh, Err("offline".into())));
    assert_eq!(model.entries.len(), 2);
    model.update(Message::Refresh);
    let refresh = model.loading.unwrap();
    model.update(Message::Catalog(refresh, crate::books::Snapshot::default()));
    model.update(Message::CatalogFinished(refresh, Ok(())));
    assert!(model.entries.is_empty());
}

#[test]
fn list_offset_keeps_the_selection_visible_with_a_margin() {
    use view::offset;
    // A list that fits never scrolls.
    assert_eq!(offset(0, 0, 3, 10), 0);
    // Moving inside the viewport leaves the offset alone.
    assert_eq!(offset(0, 5, 100, 10), 0);
    // Approaching an edge scrolls by the margin, not to the edge.
    assert_eq!(offset(0, 8, 100, 10), 1);
    assert_eq!(offset(20, 20, 100, 10), 18);
    // The last page and the first are clamped, never overscrolled.
    assert_eq!(offset(95, 99, 100, 10), 90);
    assert_eq!(offset(50, 0, 100, 10), 0);
    // A viewport that vanished on resize cannot panic or scroll.
    assert_eq!(offset(7, 3, 100, 0), 0);
}

#[test]
fn destination_rules_are_shared_by_the_dialog_and_the_key_handler() {
    let mut model = Model::new(options());
    let entry = Entry::new(
        "b.acsm".into(),
        String::new(),
        "ACSM",
        Source::Local("/b.acsm".into()),
    );
    // An unchecked device is blocked before the entry is considered.
    assert!(matches!(
        model.destination(&entry, Place::Kobo),
        Destination::Blocked("unavailable", _)
    ));
    assert_eq!(
        model.destination(&entry, Place::Local),
        Destination::Ready("fulfil ACSM")
    );
    model.catalog.status = vec![(Place::Kobo, "Ready (0 books, 0 unreadable)".into())];
    assert!(matches!(
        model.destination(&entry, Place::Kobo),
        Destination::Blocked("local first", _)
    ));
    // What the dialog dims, the key handler refuses, with the same explanation.
    let reason = match model.destination(&entry, Place::Kobo) {
        Destination::Blocked(_, reason) => reason,
        Destination::Ready(_) => unreachable!(),
    };
    model.entries = vec![entry.clone()];
    model.update(key(KeyCode::Enter));
    assert!(!model.action.is_empty());
    assert!(model.update(key(KeyCode::Char('2'))).is_empty());
    assert!(model.busy.is_none());
    assert_eq!(model.status, reason);
    // Local remains offered, and starting work blocks every destination.
    assert!(matches!(
        model.update(key(KeyCode::Char('1'))).as_slice(),
        [Effect::Work { .. }]
    ));
    for place in [Place::Local, Place::Kobo, Place::Xteink] {
        assert!(matches!(
            model.destination(&entry, place),
            Destination::Blocked("busy", _)
        ));
    }
}

#[test]
fn marking_filters_and_bulk_copies_act_on_the_set() {
    let mut model = Model::new(options());
    model.update(Message::Refresh);
    let id = model.loading.unwrap();
    model.update(Message::Catalog(
        id,
        crate::books::Snapshot {
            acsm: vec!["/a.acsm".into(), "/b.acsm".into(), "/c.acsm".into()],
            status: vec![(Place::Local, "Ready (0 books, 0 unreadable)".into())],
            ..Default::default()
        },
    ));
    model.update(Message::CatalogFinished(id, Ok(())));
    assert_eq!(model.filtered().len(), 3);
    // Marking follows the highlighted row and survives moving away from it.
    model.update(key(KeyCode::Char(' ')));
    model.update(key(KeyCode::Down));
    model.update(key(KeyCode::Char(' ')));
    assert_eq!(model.marked.len(), 2);
    assert!(model.is_marked(&model.entries[0]));
    // Enter acts on the marked set rather than the highlighted row alone.
    model.update(key(KeyCode::Enter));
    assert_eq!(model.action.len(), 2);
    let effects = model.update(key(KeyCode::Char('1')));
    assert!(
        matches!(effects.as_slice(), [Effect::Work { entries, target, .. }]
        if entries.len() == 2 && *target == Place::Local)
    );
    // Starting a copy consumes the marks, and Escape stops the job in progress.
    assert!(model.marked.is_empty());
    assert!(model.action.is_empty());
    let job = model.busy.unwrap();
    model.update(key(KeyCode::Esc));
    assert!(model.status.starts_with("Stopping"));
    assert!(model.busy.is_some());
    model.update(Message::Finished(job, Ok("Copied 1 of 2".into())));
    // A filter narrows the same list; an ACSM is only ever an import request.
    model.update(key(KeyCode::Char('f')));
    assert_eq!(model.filter.label(), "Missing from Local");
    assert!(model.filtered().is_empty());
    model.update(key(KeyCode::Char('a')));
    assert!(model.marked.is_empty());
    // Shift-F walks back, so overshooting costs one key rather than a lap.
    model.update(key(KeyCode::Char('F')));
    assert_eq!(model.filter.label(), "All books");
    model.update(key(KeyCode::Char('F')));
    assert_eq!(model.filter.label(), "Unreadable");
    // A full lap forward, in order, ending where it started.
    for expected in [
        "All books",
        "Missing from Local",
        "Missing from Kobo",
        "Missing from Xteink",
        "Only on Xteink",
        "Unreadable",
    ] {
        model.update(key(KeyCode::Char('f')));
        assert_eq!(model.filter.label(), expected);
    }
    model.update(key(KeyCode::Char('f')));
    assert_eq!(model.filter.label(), "All books");
    // Mark-all covers everything shown, and repeating it clears the set.
    model.update(key(KeyCode::Char('a')));
    assert_eq!(model.marked.len(), 3);
    model.update(key(KeyCode::Char('a')));
    assert!(model.marked.is_empty());
}

#[test]
fn an_untouched_selection_stays_at_the_top_while_books_stream_in() {
    let mut model = Model::new(options());
    model.update(Message::Refresh);
    let id = model.loading.unwrap();
    // The walk reports a pending ACSM before any book has been read.
    model.update(Message::Catalog(
        id,
        crate::books::Snapshot {
            acsm: vec!["/pending.acsm".into()],
            ..Default::default()
        },
    ));
    assert_eq!(model.selected, 0);
    let arriving = |titles: &[&str]| crate::books::Snapshot {
        acsm: vec!["/pending.acsm".into()],
        books: titles
            .iter()
            .map(|title| crate::books::Book {
                title: (*title).into(),
                author: "Author".into(),
                copies: vec![],
            })
            .collect(),
        ..Default::default()
    };
    // Books sort above the ACSM; an untouched selection must not follow it down.
    model.update(Message::Catalog(id, arriving(&["Dune"])));
    assert_eq!(model.selected, 0);
    model.update(Message::Catalog(id, arriving(&["Dune", "Piranesi"])));
    assert_eq!(model.selected, 0);
    assert_eq!(model.filtered()[model.selected].title, "Dune");
    // Once the reader moves, their choice is kept as the list grows.
    model.update(key(KeyCode::End));
    assert_eq!(model.filtered()[model.selected].title, "pending.acsm");
    model.update(Message::Catalog(
        id,
        arriving(&["Dune", "Neuromancer", "Piranesi"]),
    ));
    assert_eq!(model.filtered()[model.selected].title, "pending.acsm");
}

#[test]
fn titles_truncate_by_printed_width_not_character_count() {
    use view::shorten;
    assert_eq!(shorten("Dune", 10), "Dune");
    assert_eq!(shorten("aaaaaaa", 4), "aaa…");
    // A CJK glyph occupies two cells, so half as many fit in a column.
    assert_eq!(shorten("世界の終わり", 6), "世界…");
    assert_eq!(shorten("世界の終わり", 12), "世界の終わり");
    // Control characters are still replaced before anything is measured.
    assert_eq!(shorten("a\u{1b}b", 8), "a b");
}

#[test]
fn a_running_set_reports_how_far_it_has_come() {
    let mut model = Model::new(options());
    model.entries = (0..3)
        .map(|i| {
            Entry::new(
                format!("Book {i}"),
                "Writer".into(),
                "ACSM",
                Source::Local(format!("/{i}.acsm").into()),
            )
        })
        .collect();
    model.catalog.status = vec![(Place::Local, "Ready (0 books, 0 unreadable)".into())];
    model.update(key(KeyCode::Char('a')));
    model.update(key(KeyCode::Enter));
    assert!(matches!(
        model.update(key(KeyCode::Char('1'))).as_slice(),
        [Effect::Work { entries, .. }] if entries.len() == 3
    ));
    let job = model.busy.unwrap();
    assert_eq!(model.step, None);
    model.update(Message::Step(job, 1, 3));
    assert_eq!(model.step, Some((1, 3)));
    // The book's own progress replaces the text but never the count.
    model.update(Message::Progress(job, "Sending to CrossPoint…".into()));
    assert_eq!(model.step, Some((1, 3)));
    assert_eq!(model.status, "Sending to CrossPoint…");
    // A stale job cannot move the bar, and finishing clears it.
    model.update(Message::Step(job + 1, 9, 9));
    assert_eq!(model.step, Some((1, 3)));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| view::draw(&model, frame)).unwrap();
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("1 of 3 books"), "{rendered}");
    model.update(Message::Finished(job, Ok("Copied 3 of 3".into())));
    assert_eq!(model.step, None);
    // A single book needs no bar.
    model.busy = Some(job);
    model.update(Message::Step(job, 0, 1));
    assert_eq!(model.step, None);
}
