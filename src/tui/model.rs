use super::{Entry, Options, Source};
use crate::books::{Place, Snapshot};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
#[derive(Debug)]
pub(super) enum Message {
    Input(Event),
    InputError(String),
    Refresh,
    Tick,
    Catalog(u64, Snapshot),
    CatalogFinished(u64, Result<(), String>),
    Progress(u64, String),
    Finished(u64, Result<String, String>),
}
pub(super) enum Effect {
    Load {
        id: u64,
        options: Box<Options>,
    },
    Work {
        id: u64,
        options: Box<Options>,
        entry: Entry,
    },
    Quit,
}
/// Whether a copy to one destination may start. The dialog renders these and
/// the key handler enforces them, so a shown option and an accepted key agree.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Destination {
    /// Short hint for an allowed copy.
    Ready(&'static str),
    /// Short dialog label and the full explanation for the status line.
    Blocked(&'static str, String),
}
pub(super) struct Model {
    pub options: Options,
    pub entries: Vec<Entry>,
    pub query: String,
    pub selected: usize,
    pub search: bool,
    pub busy: Option<u64>,
    pub loading: Option<u64>,
    pub pending_quit: bool,
    pub status: String,
    pub tick: usize,
    pub catalog: Snapshot,
    pub action: Option<Entry>,
    /// Layout-only: the list offset the last frame settled on. Rendering may
    /// adjust it to keep the selection visible; it holds no operation state.
    pub scroll: std::cell::Cell<usize>,
    retaining: bool,
    pending_catalog: Option<Snapshot>,
    next_id: u64,
}
impl Model {
    pub fn new(options: Options) -> Self {
        Self {
            options,
            entries: vec![],
            query: String::new(),
            selected: 0,
            search: false,
            busy: None,
            loading: None,
            pending_quit: false,
            status: "Discovering books…".into(),
            tick: 0,
            catalog: Snapshot::default(),
            action: None,
            scroll: std::cell::Cell::new(0),
            retaining: false,
            pending_catalog: None,
            next_id: 0,
        }
    }
    pub fn filtered(&self) -> Vec<&Entry> {
        let query = self.query.to_lowercase();
        self.entries
            .iter()
            .filter(|e| {
                format!("{} {}", e.title, e.author)
                    .to_lowercase()
                    .contains(&query)
            })
            .collect()
    }
    /// The catalog's version of a book, so a stale entry cannot hide a copy
    /// that another location has since reported.
    fn current<'a>(&'a self, book: &'a crate::books::Book) -> &'a crate::books::Book {
        self.catalog
            .books
            .iter()
            .find(|b| {
                b.copies.iter().any(|c| {
                    book.copies
                        .iter()
                        .any(|d| c.place == d.place && c.path == d.path)
                })
            })
            .unwrap_or(book)
    }
    pub fn destination(&self, entry: &Entry, target: Place) -> Destination {
        if self.busy.is_some() {
            return Destination::Blocked("busy", "Wait for the current copy to finish.".into());
        }
        if self.retaining {
            return Destination::Blocked(
                "refreshing",
                "Refreshing locations; wait before copying from the previous inventory.".into(),
            );
        }
        if target != Place::Local && !self.catalog.ready(target) {
            return Destination::Blocked(
                "unavailable",
                format!("{} is unavailable or still being checked.", target.label()),
            );
        }
        match &entry.source {
            Source::Local(_) if target != Place::Local => Destination::Blocked(
                "local first",
                "Import the ACSM locally first, then refresh to copy its EPUB.".into(),
            ),
            Source::Local(_) => Destination::Ready("fulfil ACSM"),
            Source::Book(book, _) => {
                let book = self.current(book);
                if book.has(target) {
                    return Destination::Blocked(
                        "already here",
                        format!("A copy is already in {}.", target.label()),
                    );
                }
                if self.loading.is_some()
                    && book
                        .preferred()
                        .is_some_and(|c| c.optimized || c.place == Place::Xteink)
                {
                    return Destination::Blocked(
                        "wait for discovery",
                        "Wait for discovery to finish so an available original can be preferred."
                            .into(),
                    );
                }
                Destination::Ready(if target == Place::Xteink {
                    "copy, optimized"
                } else {
                    "copy"
                })
            }
        }
    }
    fn selected_entry(&self) -> Option<Entry> {
        self.filtered().get(self.selected).map(|e| (*e).clone())
    }
    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }
    fn quit(&mut self) -> Vec<Effect> {
        if self.busy.is_some() || self.loading.is_some() {
            self.pending_quit = true;
            self.status = "Finishing current work before quitting…".into();
            vec![]
        } else {
            vec![Effect::Quit]
        }
    }
    pub fn update(&mut self, message: Message) -> Vec<Effect> {
        match message {
            Message::Refresh if !self.pending_quit => {
                if self.loading.is_some() || self.busy.is_some() {
                    self.status = "Wait for current work before refreshing.".into();
                    return vec![];
                }
                self.action = None;
                self.retaining = !self.entries.is_empty();
                self.pending_catalog = None;
                let id = self.id();
                self.loading = Some(id);
                self.status = "Discovering Local, Kobo and Xteink independently…".into();
                return vec![Effect::Load {
                    id,
                    options: Box::new(self.options.clone()),
                }];
            }
            Message::Catalog(id, snapshot) if self.loading == Some(id) => {
                if self.retaining {
                    self.pending_catalog = Some(snapshot);
                    return vec![];
                }
                let selected = self.selected_entry();
                self.entries = snapshot
                    .books
                    .iter()
                    .map(|b| Entry {
                        title: b.title.clone(),
                        author: b.author.clone(),
                        kind: "EPUB",
                        source: Source::Book(Box::new(b.clone()), Place::Local),
                    })
                    .collect();
                self.entries.extend(snapshot.acsm.iter().map(|p| {
                    Entry {
                        title: p
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                        author: String::new(),
                        kind: "ACSM",
                        source: Source::Local(p.clone()),
                    }
                }));
                self.catalog = snapshot;
                self.selected = selected
                    .and_then(|old| {
                        self.filtered()
                            .iter()
                            .position(|e| match (&old.source, &e.source) {
                                (Source::Book(a, _), Source::Book(b, _)) => {
                                    a.copies.iter().any(|c| {
                                        b.copies
                                            .iter()
                                            .any(|d| c.place == d.place && c.path == d.path)
                                    })
                                }
                                (a, b) => a == b,
                            })
                    })
                    .unwrap_or(0);
            }
            Message::CatalogFinished(id, result) if self.loading == Some(id) => {
                self.retaining = false;
                if result.is_ok() {
                    if let Some(snapshot) = self.pending_catalog.take() {
                        self.update(Message::Catalog(id, snapshot));
                    }
                } else {
                    self.pending_catalog = None;
                    for (_, status) in &mut self.catalog.status {
                        *status = "Unavailable: refresh failed; showing previous inventory".into();
                    }
                }
                self.loading = None;
                if self.busy.is_none() {
                    self.status=result.map(|_|"Library ready. Enter chooses a copy destination; r refreshes locations.".into()).unwrap_or_else(|e|format!("Discovery error: {e}"));
                }
            }
            Message::Progress(id, s) if self.busy == Some(id) => self.status = s,
            Message::Finished(id, result) if self.busy == Some(id) => {
                self.busy = None;
                self.status = result.unwrap_or_else(|e| format!("Error: {e}"));
            }
            Message::Tick => self.tick = self.tick.wrapping_add(1),
            Message::InputError(e) => {
                self.status = e;
                return self.quit();
            }
            Message::Input(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    return self.quit();
                }
                if self.pending_quit {
                    return vec![];
                }
                if let Some(mut entry) = self.action.clone() {
                    let target = match key.code {
                        KeyCode::Char('1') => Some(Place::Local),
                        KeyCode::Char('2') => Some(Place::Kobo),
                        KeyCode::Char('3') => Some(Place::Xteink),
                        KeyCode::Esc | KeyCode::Char('q') => {
                            self.action = None;
                            return vec![];
                        }
                        _ => None,
                    };
                    if let Some(target) = target {
                        if let Destination::Blocked(_, reason) = self.destination(&entry, target) {
                            self.status = reason;
                            return vec![];
                        }
                        if let Source::Book(book, t) = &mut entry.source {
                            if let Some(current) = self.catalog.books.iter().find(|b| {
                                b.copies.iter().any(|c| {
                                    book.copies
                                        .iter()
                                        .any(|d| c.place == d.place && c.path == d.path)
                                })
                            }) {
                                **book = current.clone();
                            }
                            *t = target;
                        }
                        let id = self.id();
                        self.busy = Some(id);
                        self.action = None;
                        let mut options = self.options.clone();
                        if !matches!(entry.source, Source::Book(_, _)) {
                            options.send_to = None;
                            options.copy_to = None;
                        }
                        return vec![Effect::Work {
                            id,
                            options: Box::new(options),
                            entry,
                        }];
                    }
                    return vec![];
                }
                if self.search {
                    match key.code {
                        KeyCode::Down => {
                            self.selected =
                                (self.selected + 1).min(self.filtered().len().saturating_sub(1))
                        }
                        KeyCode::Up => self.selected = self.selected.saturating_sub(1),
                        KeyCode::Home => self.selected = 0,
                        KeyCode::End => self.selected = self.filtered().len().saturating_sub(1),
                        KeyCode::PageDown => {
                            self.selected =
                                (self.selected + 10).min(self.filtered().len().saturating_sub(1))
                        }
                        KeyCode::PageUp => self.selected = self.selected.saturating_sub(10),
                        KeyCode::Esc | KeyCode::Enter => self.search = false,
                        KeyCode::Backspace => {
                            self.query.pop();
                            self.selected = 0;
                        }
                        KeyCode::Char(c) => {
                            self.query.push(c);
                            self.selected = 0;
                        }
                        _ => {}
                    };
                    return vec![];
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return self.quit(),
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.selected =
                            (self.selected + 1).min(self.filtered().len().saturating_sub(1))
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.selected = self.selected.saturating_sub(1)
                    }
                    KeyCode::Home => self.selected = 0,
                    KeyCode::End => self.selected = self.filtered().len().saturating_sub(1),
                    KeyCode::PageDown => {
                        self.selected =
                            (self.selected + 10).min(self.filtered().len().saturating_sub(1))
                    }
                    KeyCode::PageUp => self.selected = self.selected.saturating_sub(10),
                    KeyCode::Char('/') => self.search = true,
                    KeyCode::Char('r') => return self.update(Message::Refresh),
                    KeyCode::Enter if self.busy.is_none() => {
                        self.action = self.selected_entry();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        if self.pending_quit && self.busy.is_none() && self.loading.is_none() {
            return vec![Effect::Quit];
        }
        vec![]
    }
}
