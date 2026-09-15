use super::{Entry, Options, Source};
use crate::books::{books, Place, Snapshot};
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
    /// Books finished and books in the running job.
    Step(u64, usize, usize),
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
        entries: Vec<Entry>,
        target: Place,
        /// Set to stop a multi-book copy after the book in progress.
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    },
    Quit,
}
/// The questions worth asking of a library, in the order they are cycled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Filter {
    All,
    Missing(Place),
    Only(Place),
    Unreadable,
}
impl Filter {
    pub fn label(self) -> String {
        match self {
            Self::All => "All books".into(),
            Self::Missing(place) => format!("Missing from {}", place.label()),
            Self::Only(place) => format!("Only on {}", place.label()),
            Self::Unreadable => "Unreadable".into(),
        }
    }
    fn next(self) -> Self {
        match self {
            Self::All => Self::Missing(Place::Kobo),
            Self::Missing(Place::Kobo) => Self::Missing(Place::Xteink),
            Self::Missing(_) => Self::Only(Place::Xteink),
            Self::Only(_) => Self::Unreadable,
            Self::Unreadable => Self::All,
        }
    }
    fn keeps(self, entry: &Entry) -> bool {
        let Source::Book(book) = &entry.source else {
            // An ACSM is a pending import, not a book that is somewhere.
            return self == Self::All;
        };
        match self {
            Self::All => true,
            Self::Missing(place) => !book.has(place),
            Self::Only(place) => book.copies.iter().all(|copy| copy.place == place),
            Self::Unreadable => book.copies.iter().any(|copy| copy.sha.is_empty()),
        }
    }
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
    /// How far a running job has come, once it has more than one book.
    pub step: Option<(usize, usize)>,
    pub catalog: Snapshot,
    /// The books a copy dialog is open for: the highlighted one, or the marked
    /// set. Empty means no dialog.
    pub action: Vec<Entry>,
    pub filter: Filter,
    /// Whether the selection is the reader's own. Until it is, arriving books
    /// must not drag it along: discovery reorders the list as it streams.
    touched: bool,
    /// One copy of each marked book, so a refresh can find it again.
    pub marked: Vec<(Place, String)>,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Layout-only: the list offset the last frame settled on. Rendering may
    /// adjust it to keep the selection visible; it holds no operation state.
    pub scroll: std::cell::Cell<usize>,
    /// Layout-only: the width the last frame had, for sizing the progress bar.
    pub width: std::cell::Cell<u16>,
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
            step: None,
            catalog: Snapshot::default(),
            action: vec![],
            filter: Filter::All,
            touched: false,
            marked: vec![],
            cancel: None,
            scroll: std::cell::Cell::new(0),
            width: std::cell::Cell::new(80),
            retaining: false,
            pending_catalog: None,
            next_id: 0,
        }
    }
    pub fn filtered(&self) -> Vec<&Entry> {
        let query = self.query.to_lowercase();
        self.entries
            .iter()
            .filter(|e| self.filter.keeps(e))
            .filter(|e| e.search.contains(&query))
            .collect()
    }
    /// A book is marked when it still holds a copy that was marked earlier, so
    /// a refresh that adds or removes copies does not lose the set.
    pub fn is_marked(&self, entry: &Entry) -> bool {
        match &entry.source {
            Source::Book(book) => book.copies.iter().any(|c| {
                self.marked
                    .iter()
                    .any(|(p, path)| *p == c.place && *path == c.path)
            }),
            Source::Local(path) => self
                .marked
                .iter()
                .any(|(p, marked)| *p == Place::Local && marked == &path.display().to_string()),
        }
    }
    fn mark(&mut self, entry: &Entry) {
        if self.is_marked(entry) {
            let copies: Vec<_> = match &entry.source {
                Source::Book(book) => book
                    .copies
                    .iter()
                    .map(|c| (c.place, c.path.clone()))
                    .collect(),
                Source::Local(path) => vec![(Place::Local, path.display().to_string())],
            };
            self.marked
                .retain(|marked| !copies.iter().any(|copy| copy == marked));
            return;
        }
        match &entry.source {
            Source::Book(book) => {
                if let Some(copy) = book.preferred().or_else(|| book.copies.first()) {
                    self.marked.push((copy.place, copy.path.clone()));
                }
            }
            Source::Local(path) => self.marked.push((Place::Local, path.display().to_string())),
        }
    }
    /// The marked books, or the highlighted one when nothing is marked.
    fn chosen(&self) -> Vec<Entry> {
        let marked: Vec<Entry> = self
            .entries
            .iter()
            .filter(|entry| self.is_marked(entry))
            .cloned()
            .collect();
        if marked.is_empty() {
            self.selected_entry().into_iter().collect()
        } else {
            marked
        }
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
            Source::Book(book) => {
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
                self.action.clear();
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
                let selected = self.touched.then(|| self.selected_entry()).flatten();
                self.entries = snapshot
                    .books
                    .iter()
                    .map(|b| {
                        Entry::new(
                            b.title.clone(),
                            b.author.clone(),
                            "EPUB",
                            Source::Book(Box::new(b.clone())),
                        )
                    })
                    .collect();
                self.entries.extend(snapshot.acsm.iter().map(|p| {
                    Entry::new(
                        p.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                        String::new(),
                        "ACSM",
                        Source::Local(p.clone()),
                    )
                }));
                self.catalog = snapshot;
                self.selected = selected
                    .and_then(|old| {
                        self.filtered()
                            .iter()
                            .position(|e| match (&old.source, &e.source) {
                                (Source::Book(a), Source::Book(b)) => a.copies.iter().any(|c| {
                                    b.copies
                                        .iter()
                                        .any(|d| c.place == d.place && c.path == d.path)
                                }),
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
            Message::Step(id, done, total) if self.busy == Some(id) => {
                self.step = (total > 1).then_some((done, total));
            }
            Message::Finished(id, result) if self.busy == Some(id) => {
                self.busy = None;
                self.cancel = None;
                self.step = None;
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
                if !self.action.is_empty() {
                    let target = match key.code {
                        KeyCode::Char('1') => Some(Place::Local),
                        KeyCode::Char('2') => Some(Place::Kobo),
                        KeyCode::Char('3') => Some(Place::Xteink),
                        KeyCode::Esc | KeyCode::Char('q') => {
                            self.action.clear();
                            return vec![];
                        }
                        _ => None,
                    };
                    let Some(target) = target else {
                        return vec![];
                    };
                    // Books that cannot be copied are left behind with their
                    // reason; a set is never refused as a whole for one of them.
                    let mut blocked = None;
                    let entries: Vec<Entry> = self
                        .action
                        .clone()
                        .into_iter()
                        .filter(|entry| match self.destination(entry, target) {
                            Destination::Ready(_) => true,
                            Destination::Blocked(_, reason) => {
                                blocked.get_or_insert(reason);
                                false
                            }
                        })
                        .map(|mut entry| {
                            // Use the catalog's copies, not those captured when
                            // the dialog opened.
                            if let Source::Book(book) = &mut entry.source {
                                **book = self.current(book).clone();
                            }
                            entry
                        })
                        .collect();
                    if entries.is_empty() {
                        self.status =
                            blocked.unwrap_or_else(|| "Nothing left to copy there.".into());
                        return vec![];
                    }
                    let skipped = self.action.len() - entries.len();
                    self.action.clear();
                    self.marked.clear();
                    let id = self.id();
                    self.busy = Some(id);
                    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                    self.cancel = Some(cancel.clone());
                    self.status = format!(
                        "Copying {} to {}…{}",
                        books(entries.len()),
                        target.label(),
                        if skipped > 0 {
                            format!(" {skipped} skipped.")
                        } else {
                            String::new()
                        }
                    );
                    let mut options = self.options.clone();
                    if entries
                        .iter()
                        .all(|entry| !matches!(entry.source, Source::Book(_)))
                    {
                        options.send_to = None;
                        options.copy_to = None;
                    }
                    return vec![Effect::Work {
                        id,
                        options: Box::new(options),
                        entries,
                        target,
                        cancel,
                    }];
                }
                if self.search {
                    self.touched |= matches!(
                        key.code,
                        KeyCode::Down
                            | KeyCode::Up
                            | KeyCode::Home
                            | KeyCode::End
                            | KeyCode::PageDown
                            | KeyCode::PageUp
                    );
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
                            self.touched = false;
                        }
                        KeyCode::Char(c) => {
                            self.query.push(c);
                            self.selected = 0;
                            self.touched = false;
                        }
                        _ => {}
                    };
                    return vec![];
                }
                self.touched |= matches!(
                    key.code,
                    KeyCode::Down
                        | KeyCode::Up
                        | KeyCode::Char('j')
                        | KeyCode::Char('k')
                        | KeyCode::Home
                        | KeyCode::End
                        | KeyCode::PageDown
                        | KeyCode::PageUp
                );
                match key.code {
                    KeyCode::Esc if self.busy.is_some() => {
                        if let Some(cancel) = &self.cancel {
                            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                            self.status = "Stopping after the book in progress…".into();
                        }
                    }
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
                    KeyCode::Char(' ') => {
                        if let Some(entry) = self.selected_entry() {
                            self.mark(&entry);
                        }
                    }
                    KeyCode::Char('a') => {
                        let shown: Vec<Entry> = self.filtered().into_iter().cloned().collect();
                        let all = !shown.is_empty() && shown.iter().all(|e| self.is_marked(e));
                        for entry in shown {
                            if all == self.is_marked(&entry) {
                                self.mark(&entry);
                            }
                        }
                    }
                    KeyCode::Char('f') => {
                        self.filter = self.filter.next();
                        self.selected = 0;
                        self.touched = false;
                        self.scroll.set(0);
                    }
                    KeyCode::Enter if self.busy.is_none() => {
                        self.action = self.chosen();
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
