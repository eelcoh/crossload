use super::*;
use crate::books::Place;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use model::Destination;
use std::sync::{Arc, Mutex};
fn options() -> Options {
    Options {
        config: "/unused/config.json".into(),
        first_run: false,
        show_previews: false,
        device: None,
        browse: "/books".into(),
        output: "/output".into(),
        state: None,
        send_to: None,
        copy_to: None,
        folder: "/".into(),
        optimize: true,
        kepub: false,
        organized: true,
        serial: None,
    }
}
fn key(code: KeyCode) -> Message {
    Message::Input(Event::Key(KeyEvent::from(code)))
}

#[test]
fn settings_save_is_atomic_for_the_model_and_uses_the_selected_config() {
    let dir = tempfile::tempdir().unwrap();
    let mut opts = options();
    opts.config = dir.path().join("custom/config.json");
    opts.browse = dir.path().into();
    let original_output = opts.output.clone();
    let mut model = Model::new(opts);
    model.update(key(KeyCode::Char(',')));
    let panel = model.settings.as_mut().unwrap();
    panel.values[1] = dir.path().join("imports").display().to_string();
    panel.selected = 7;
    let mut effects = model.update(key(KeyCode::Enter));
    assert_eq!(model.options.output, original_output);
    let Effect::Configure { id, task } = effects.remove(0) else {
        panic!("expected save")
    };
    assert!(model.update(key(KeyCode::Esc)).is_empty());
    assert!(model.settings.is_some());
    model.update(Message::Configured(id + 1, Err("stale".into())));
    assert_eq!(model.configuring, Some(id));
    let result = settings::perform(task).unwrap();
    let saved = crate::config::load(&model.options.config).unwrap();
    assert_eq!(saved.output, Some(dir.path().join("imports")));
    let effects = model.update(Message::Configured(id, Ok(result)));
    assert!(matches!(effects.as_slice(), [Effect::Load { .. }]));
    assert!(model.settings.is_none());
    assert_eq!(model.options.output, dir.path().join("imports"));

    let mut model = Model::new(options());
    model.open_settings(false);
    model.configuring = Some(1);
    model.update(Message::Configured(1, Err("Permission denied".into())));
    assert!(model
        .settings
        .as_ref()
        .unwrap()
        .message
        .contains("Permission denied"));
    assert_eq!(model.options.output, original_output);
    model.update(key(KeyCode::Esc));
    assert!(model.settings.is_none());
}

#[test]
fn first_run_can_be_skipped_and_settings_editing_handles_unicode() {
    let mut opts = options();
    opts.first_run = true;
    let (mut app, _) = App::new(opts);
    assert!(app.model.settings.as_ref().unwrap().first_run);
    assert!(app.model.loading.is_none());
    assert!(matches!(
        app.model.update(key(KeyCode::Esc)).as_slice(),
        [Effect::Load { .. }]
    ));

    let mut panel = settings::Panel::new(&options(), false);
    panel.values[0] = "/书📚".into();
    panel.input(KeyEvent::from(KeyCode::Enter));
    panel.input(KeyEvent::from(KeyCode::Left));
    panel.input(KeyEvent::from(KeyCode::Backspace));
    panel.input(KeyEvent::from(KeyCode::Char('é')));
    assert_eq!(panel.values[0], "/é📚");
    panel.input(KeyEvent::from(KeyCode::Esc));
    assert_eq!(panel.values[0], "/书📚");
    assert!(panel.edit.is_none());
    panel.values[0] = "relative".into();
    assert!(panel.defaults().is_err());
}

#[test]
fn setup_explains_each_row_and_refuses_a_books_folder_that_is_not_there() {
    let dir = tempfile::tempdir().unwrap();
    let mut opts = options();
    opts.browse = dir.path().into();
    opts.output = dir.path().join("imports");
    opts.first_run = true;
    let (mut app, _) = App::new(opts);
    // Setup holds discovery back, so nothing may claim to be scanning.
    assert!(app.model.loading.is_none());
    assert!(app.model.status.contains("Finish setup"));
    let panel = app.model.settings.as_ref().unwrap();
    assert_eq!(panel.values[3], settings::READER);
    assert_eq!(panel.marks[0], settings::Mark::Good("folder found"));
    assert_eq!(
        panel.marks[1],
        settings::Mark::Note("created on first import")
    );
    assert!(panel.hint().starts_with("Required."));

    // Ctrl+S saves from any row, and a books folder that is not there sends
    // the user back to the row that stopped the save.
    let panel = app.model.settings.as_mut().unwrap();
    panel.selected = 5;
    panel.values[0] = dir.path().join("gone").display().to_string();
    let save = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
    assert!(app
        .model
        .update(Message::Input(Event::Key(save)))
        .is_empty());
    let panel = app.model.settings.as_ref().unwrap();
    assert_eq!(panel.selected, 0);
    assert_eq!(panel.marks[0], settings::Mark::Bad("not found"));
    assert_eq!(panel.hint(), "Books folder: not found");

    // Skipping writes nothing and leaves the library saying where to look.
    let effects = app.model.update(key(KeyCode::Esc));
    assert!(matches!(effects.as_slice(), [Effect::Load { .. }]));
    assert!(app.model.skipped && !app.model.options.first_run);
    assert!(app.model.settings.is_none());
    assert!(!dir.path().join("config.json").exists());
}

#[test]
fn the_kobo_row_finds_its_own_mount_and_still_takes_a_typed_path() {
    let mut panel = settings::Panel::new(&options(), false);
    panel.selected = 2;
    // Enter on the Kobo row asks for a search instead of a path.
    assert!(matches!(
        panel.input(KeyEvent::from(KeyCode::Enter)),
        settings::Action::Run(settings::Task::Detect)
    ));
    panel.detected(vec![]);
    assert!(panel.message.contains("No mounted Kobo found"));
    assert!(panel.values[2].is_empty());
    // One match answers the row; several ask which, back on the same row.
    panel.detected(vec!["/media/kobo".into()]);
    assert_eq!(panel.values[2], "/media/kobo");
    assert_eq!((panel.selected, panel.device_selected), (2, None));
    panel.detected(vec!["/media/a".into(), "/media/b".into()]);
    assert_eq!(panel.device_selected, Some(0));
    panel.input(KeyEvent::from(KeyCode::Down));
    panel.input(KeyEvent::from(KeyCode::Enter));
    assert_eq!(panel.values[2], "/media/b");
    // e types a mount in by hand without detecting anything.
    assert!(matches!(
        panel.input(KeyEvent::from(KeyCode::Char('e'))),
        settings::Action::None
    ));
    assert!(panel.edit.is_some() && panel.selected == 2);
}

#[test]
fn a_pdf_reaches_the_reader_only_by_converting_and_only_when_that_is_worth_it() {
    use crate::pdf::{Concern, Verdict};
    let mut model = Model::new(options());
    model.catalog.status = vec![(Place::CrossPoint, "Ready".into())];
    let pdf = |verdict: Option<Verdict>| {
        let mut copy = crate::books::copy(Place::Local, "/books/paper.pdf", 100, false);
        copy.verdict = verdict;
        let book = crate::books::Book {
            title: "Paper".into(),
            author: String::new(),
            series: None,
            series_index: None,
            copies: vec![copy],
        };
        Entry::new(
            "Paper".into(),
            String::new(),
            "PDF",
            Source::Book(Box::new(book)),
        )
    };
    let ready = |model: &Model, entry| model.destination(entry, Place::CrossPoint);

    // Nothing readable in it: refused outright, with the reason.
    let entry = pdf(Some(Verdict::Impossible(Concern::NoText)));
    assert!(matches!(
        ready(&model, &entry),
        Destination::Blocked("cannot be converted", _)
    ));
    // Clean text: offered, and it says what it will do.
    let entry = pdf(Some(Verdict::Good));
    assert!(matches!(ready(&model, &entry), Destination::Ready(_)));

    // Poor: offered, but the first press asks rather than starting work.
    let entry = pdf(Some(Verdict::Poor(vec![Concern::NoChapters])));
    assert!(matches!(ready(&model, &entry), Destination::Ask(_, _)));
    model.entries = vec![entry];
    model.update(key(KeyCode::Enter));
    assert!(model.update(key(KeyCode::Char('3'))).is_empty());
    let asked = model.ask.clone().expect("should have asked first");
    assert_eq!(asked.0, Place::CrossPoint);
    assert!(asked.1.contains("no chapters"), "{}", asked.1);
    // Anything but a yes leaves the question standing or withdraws it.
    model.update(key(KeyCode::Esc));
    assert!(model.ask.is_none() && !model.action.is_empty());
    model.update(key(KeyCode::Char('3')));
    assert!(matches!(
        model.update(key(KeyCode::Char('y'))).as_slice(),
        [Effect::Work {
            target: Place::CrossPoint,
            ..
        }]
    ));
    assert!(model.ask.is_none());
}

#[test]
fn a_series_sorts_in_its_own_order_and_a_field_can_be_searched_alone() {
    let volume = |title: &str, series: Option<(&str, f32)>| {
        let book = crate::books::Book {
            title: title.into(),
            author: "Mick Herron".into(),
            series: series.map(|(name, _)| name.to_owned()),
            series_index: series.map(|(_, index)| index),
            copies: vec![crate::books::copy(Place::Local, "/b.epub", 1, false)],
        };
        Entry::new(
            title.into(),
            "Mick Herron".into(),
            "EPUB",
            Source::Book(Box::new(book)),
        )
    };
    let mut model = Model::new(options());
    model.entries = vec![
        volume("Standalone", None),
        volume("Tenth volume", Some(("Slough House", 10.0))),
        volume("Second volume", Some(("Slough House", 2.0))),
        volume("An interlude", Some(("Slough House", 2.5))),
    ];
    // Title order first, which is what interleaves a series with everything.
    model.update(key(KeyCode::Char('s')));
    assert_eq!(model.sort, model::Sort::Author);
    model.update(key(KeyCode::Char('s')));
    assert_eq!(model.sort, model::Sort::Series);
    let order: Vec<&str> = model.filtered().iter().map(|e| e.title.as_str()).collect();
    // Two before ten rather than beside it, a half number in between, and a
    // book belonging to no series after every book that does.
    assert_eq!(
        order,
        [
            "Second volume",
            "An interlude",
            "Tenth volume",
            "Standalone"
        ]
    );

    // A named field searches only that field.
    model.query = "series:slough".into();
    assert_eq!(model.filtered().len(), 3);
    model.query = "author:herron".into();
    assert_eq!(model.filtered().len(), 4);
    model.query = "title:interlude".into();
    assert_eq!(model.filtered().len(), 1);
    model.query = "title:herron".into();
    assert!(model.filtered().is_empty());
}

#[test]
fn the_copy_dialog_says_what_room_is_left_and_when_a_set_will_not_fit() {
    let mut model = Model::new(options());
    model.catalog.status = vec![
        (Place::Kobo, "Ready".into()),
        (Place::CrossPoint, "Ready".into()),
    ];
    let book = crate::books::Book {
        title: "Dune".into(),
        author: "Frank Herbert".into(),
        series: None,
        series_index: None,
        copies: vec![crate::books::copy(
            Place::Local,
            "/books/dune.epub",
            900,
            false,
        )],
    };
    model.action = vec![Entry::new(
        "Dune".into(),
        "Frank Herbert".into(),
        "EPUB",
        Source::Book(Box::new(book)),
    )];
    let draw = |model: &Model| {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| view::draw(model, frame)).unwrap();
        format!("{:?}", terminal.backend().buffer())
    };

    // A Kobo with room says so; a reader over Wi-Fi cannot be measured at all,
    // and says that rather than leaving room to be assumed.
    model.room = vec![
        (Place::Kobo, Some(4096)),
        (Place::CrossPoint, None),
        (Place::Local, None),
    ];
    let rendered = draw(&model);
    assert!(rendered.contains("4.0 KiB free"), "{rendered}");
    assert!(rendered.contains("free unknown"), "{rendered}");

    // A destination with less room than the set weighs is marked, not hidden.
    model.room = vec![(Place::Kobo, Some(100)), (Place::CrossPoint, None)];
    let rendered = draw(&model);
    assert!(rendered.contains("✗ 100 B free"), "{rendered}");
}

#[test]
fn menus_isolate_keys_and_sort_keeps_selection_and_marks() {
    let mut model = Model::new(options());
    model.entries = vec![
        Entry::new(
            "Alpha".into(),
            "Zed".into(),
            "EPUB",
            Source::Local("/a".into()),
        ),
        Entry::new(
            "Beta".into(),
            "Amy".into(),
            "EPUB",
            Source::Local("/b".into()),
        ),
    ];
    model.update(key(KeyCode::Char(' ')));
    model.update(key(KeyCode::Char('s')));
    assert_eq!(model.filtered()[0].title, "Beta");
    assert_eq!(model.filtered()[model.selected].title, "Alpha");
    assert!(model.is_marked(model.filtered()[model.selected]));
    model.update(key(KeyCode::Char('?')));
    model.update(key(KeyCode::Enter));
    assert!(model.action.is_empty());
    model.update(key(KeyCode::Char('q')));
    assert!(!model.help);
    model.update(key(KeyCode::Char('f')));
    model.update(key(KeyCode::Down));
    model.update(key(KeyCode::Esc));
    assert_eq!(model.filter, model::Filter::All);
    assert!(model.filter_menu.is_none());
    model.update(key(KeyCode::Char('/')));
    model.update(key(KeyCode::Char('s')));
    assert_eq!(model.query, "s");
    assert_eq!(model.sort, model::Sort::Author);
}

#[test]
fn settings_help_and_filters_render_in_small_terminals() {
    for (w, h) in [(1, 1), (25, 10), (52, 20), (80, 24)] {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        let mut model = Model::new(options());
        model.help = true;
        terminal.draw(|frame| view::draw(&model, frame)).unwrap();
        model.help = false;
        model.filter_menu = Some(5);
        terminal.draw(|frame| view::draw(&model, frame)).unwrap();
        model.filter_menu = None;
        model.open_settings(false);
        for selected in 0..8 {
            model.settings.as_mut().unwrap().selected = selected;
            terminal.draw(|frame| view::draw(&model, frame)).unwrap();
        }
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
        other => unreachable!("{other:?}"),
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
    for place in [Place::Local, Place::Kobo, Place::CrossPoint] {
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
    assert_eq!(model.filter.label(), "All books");
    model.update(key(KeyCode::Down));
    model.update(key(KeyCode::Enter));
    assert_eq!(model.filter.label(), "Missing from Local");
    assert!(model.filtered().is_empty());
    model.update(key(KeyCode::Char('a')));
    assert!(model.marked.is_empty());
    // The menu supports direct selection, without cycling through filters.
    for (index, expected) in [
        "All books",
        "Missing from Local",
        "Missing from Kobo",
        "Missing from CrossPoint",
        "Only on CrossPoint",
        "Unreadable",
    ]
    .into_iter()
    .enumerate()
    {
        model.update(key(KeyCode::Char('f')));
        model.update(key(KeyCode::Char(char::from(b'1' + index as u8))));
        assert_eq!(model.filter.label(), expected);
    }
    model.update(key(KeyCode::Char('f')));
    model.update(key(KeyCode::Char('1')));
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
                series: None,
                series_index: None,
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

fn shelf(copies: Vec<crate::books::Copy>) -> Entry {
    Entry::new(
        "Slough House".into(),
        "Mick Herron".into(),
        "EPUB",
        Source::Book(Box::new(crate::books::Book {
            title: "Slough House".into(),
            author: "Mick Herron".into(),
            series: None,
            series_index: None,
            copies,
        })),
    )
}
#[test]
fn deleting_a_copy_needs_the_dialog_a_choice_and_a_confirmation() {
    let mut model = Model::new(options());
    model.entries = vec![shelf(vec![
        crate::books::copy(Place::Local, "/books/original.epub", 17_800_000, false),
        crate::books::copy(Place::Local, "/books/device.epub", 11_900_000, false),
    ])];
    model.catalog.books = match &model.entries[0].source {
        Source::Book(book) => vec![(**book).clone()],
        Source::Local(_) => unreachable!(),
    };
    // The dialog lists the copies; nothing is chosen yet.
    model.update(key(KeyCode::Char('d')));
    assert!(model.copies.is_some());
    assert!(model.confirm.is_none());
    // Choosing asks rather than deletes, and any other key is a refusal.
    assert!(model.update(key(KeyCode::Char('2'))).is_empty());
    assert_eq!(
        model.confirm,
        Some((Place::Local, "/books/device.epub".into()))
    );
    assert!(model.update(key(KeyCode::Char('n'))).is_empty());
    assert!(model.confirm.is_none());
    assert_eq!(model.status, "Nothing was deleted.");
    assert!(model.copies.is_some(), "the list stays open");
    // Only y deletes, and only the copy that was confirmed.
    model.update(key(KeyCode::Char('2')));
    let effects = model.update(key(KeyCode::Char('y')));
    assert!(
        matches!(effects.as_slice(), [Effect::Remove { place, path, .. }]
            if *place == Place::Local && path == "/books/device.epub"),
        "expected exactly that copy to be removed"
    );
    assert!(model.busy.is_some());
    assert!(model.copies.is_none() && model.confirm.is_none());
}
#[test]
fn the_only_copy_and_a_kobo_library_book_are_never_offered() {
    let mut model = Model::new(options());
    let alone = shelf(vec![crate::books::copy(
        Place::Local,
        "/books/only.epub",
        100,
        false,
    )]);
    model.entries = vec![alone.clone()];
    model.catalog.books = match &alone.source {
        Source::Book(book) => vec![(**book).clone()],
        Source::Local(_) => unreachable!(),
    };
    model.update(key(KeyCode::Char('d')));
    assert!(model.update(key(KeyCode::Char('1'))).is_empty());
    assert!(model.confirm.is_none());
    assert!(model.status.contains("only copy"), "{}", model.status);
    // A book the Kobo database owns is the Kobo's to remove, not ours.
    let kobo = shelf(vec![
        crate::books::copy(Place::Kobo, "store-id", 100, true),
        crate::books::copy(Place::Local, "/books/copy.epub", 100, false),
    ]);
    model.entries = vec![kobo.clone()];
    model.catalog.books = match &kobo.source {
        Source::Book(book) => vec![(**book).clone()],
        Source::Local(_) => unreachable!(),
    };
    model.copies = None;
    model.update(key(KeyCode::Char('d')));
    assert!(model.update(key(KeyCode::Char('1'))).is_empty());
    assert!(model.confirm.is_none());
    assert!(model.status.contains("Kobo database"), "{}", model.status);
    // The local copy beside it is still removable.
    model.update(key(KeyCode::Char('2')));
    assert!(model.confirm.is_some());
}
