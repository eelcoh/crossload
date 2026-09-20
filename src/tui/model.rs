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
    Configured(u64, Result<super::settings::Outcome, String>),
    Corrected(u64, Result<String, String>),
}
pub(super) enum Effect {
    Correct {
        id: u64,
        task: super::edit::Task,
    },
    Configure {
        id: u64,
        task: super::settings::Task,
    },
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
    Remove {
        id: u64,
        options: Box<Options>,
        book: Box<crate::books::Book>,
        place: Place,
        path: String,
    },
    Quit,
}
/// Whether one copy may be deleted. The dialog shows these and the key handler
/// enforces them, and `books::remove` checks every one of them again.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Removal {
    Ready,
    Blocked(&'static str, String),
}
/// What the list is ordered by, cycled with one key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Sort {
    Title,
    Author,
    Series,
}
impl Sort {
    pub fn label(self) -> &'static str {
        match self {
            Self::Title => " · title ↑",
            Self::Author => " · author ↑",
            Self::Series => " · series ↑",
        }
    }
    fn next(self) -> Self {
        match self {
            Self::Title => Self::Author,
            Self::Author => Self::Series,
            Self::Series => Self::Title,
        }
    }
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
    /// Every location can be the one a book is missing from: books come back
    /// from a device as readily as they go to one.
    pub const CYCLE: [Self; 6] = [
        Self::All,
        Self::Missing(Place::Local),
        Self::Missing(Place::Kobo),
        Self::Missing(Place::CrossPoint),
        Self::Only(Place::CrossPoint),
        Self::Unreadable,
    ];
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
    /// Possible, but the result will be poor. Say so and wait for a yes.
    Ask(&'static str, String),
    /// Short dialog label and the full explanation for the status line.
    Blocked(&'static str, String),
}
pub(super) struct Model {
    /// A refresh asked for while something was already running, to be done
    /// when it finishes rather than refused.
    pub pending_refresh: bool,
    /// A book's metadata being corrected, kept apart from the book until saved.
    pub correcting: Option<super::edit::Panel>,
    /// Room left at each place, read once when the copy dialog opens. A place
    /// that cannot say has None, which the dialog shows as unknown rather than
    /// leaving the reader to assume there is room.
    pub room: Vec<(Place, Option<u64>)>,
    /// A destination that wants a yes first, and why.
    pub ask: Option<(Place, String)>,
    pub settings: Option<super::settings::Panel>,
    /// Setup was dismissed without saving, so an empty library is a question
    /// about folders rather than a fact about books.
    pub skipped: bool,
    pub configuring: Option<u64>,
    pub help: bool,
    pub help_scroll: u16,
    /// Recent copies, read when the record is opened rather than kept live.
    pub history: Option<Vec<crate::history::Entry>>,
    pub filter_menu: Option<usize>,
    pub sort: Sort,
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
    /// The book whose copies are listed, and the copy awaiting confirmation.
    pub copies: Option<Entry>,
    pub confirm: Option<(Place, String)>,
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
        // Discovery waits for setup on a first run; the status line must not
        // claim work that is not happening.
        let status = if options.first_run {
            "Finish setup to scan your library, or press Esc to skip it."
        } else {
            "Discovering books…"
        };
        Self {
            pending_refresh: false,
            correcting: None,
            room: vec![],
            ask: None,
            settings: None,
            skipped: false,
            configuring: None,
            help: false,
            help_scroll: 0,
            history: None,
            filter_menu: None,
            sort: Sort::Title,
            options,
            entries: vec![],
            query: String::new(),
            selected: 0,
            search: false,
            busy: None,
            loading: None,
            pending_quit: false,
            status: status.into(),
            tick: 0,
            step: None,
            catalog: Snapshot::default(),
            action: vec![],
            filter: Filter::All,
            copies: None,
            confirm: None,
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
            .filter(|e| match query.split_once(':') {
                // A field named before the colon searches only that field, so
                // author:herron does not also match a book about him.
                Some(("author", term)) => e.author_key.contains(term.trim_start()),
                Some(("title", term)) => e.title_key.contains(term.trim_start()),
                Some(("series", term)) => e.series_key.contains(term.trim_start()),
                _ => e.search.contains(&query),
            })
            .collect()
    }
    fn sort_entries(&mut self) {
        let sort = self.sort;
        self.entries.sort_by(|a, b| {
            (a.kind == "ACSM")
                .cmp(&(b.kind == "ACSM"))
                .then_with(|| match sort {
                    Sort::Title => a.title_key.cmp(&b.title_key),
                    Sort::Author => a.author_key.cmp(&b.author_key),
                    Sort::Series => a.series_key.cmp(&b.series_key),
                })
                .then_with(|| a.title_key.cmp(&b.title_key))
        });
    }
    /// A refresh that was asked for while something was running, now that
    /// nothing is.
    fn deferred_refresh(&mut self) -> Option<Vec<Effect>> {
        let idle = self.busy.is_none() && self.loading.is_none() && !self.pending_quit;
        (self.pending_refresh && idle).then(|| self.update(Message::Refresh))
    }
    pub fn open_settings(&mut self, first_run: bool) {
        self.settings = Some(super::settings::Panel::new(&self.options, first_run));
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
                        .is_some_and(|c| c.optimized || c.place == Place::CrossPoint)
                {
                    return Destination::Blocked(
                        "wait for discovery",
                        "Wait for discovery to finish so an available original can be preferred."
                            .into(),
                    );
                }
                // The reader stores anything but lists only EPUB, so copying
                // a PDF there would leave a file nobody can open.
                if target == Place::CrossPoint {
                    if let Some(copy) = book.preferred().filter(|c| !c.format.shown_on_reader()) {
                        use crate::{format::Format, pdf::Verdict};
                        return match (copy.format, &copy.verdict) {
                            (Format::Pdf, Some(Verdict::Good)) => {
                                Destination::Ready("convert to EPUB, copy")
                            }
                            (Format::Pdf, Some(verdict @ Verdict::Poor(_))) => Destination::Ask(
                                "converts poorly",
                                format!(
                                    "{} can become an EPUB, but {}. The PDF is kept either way.",
                                    entry.title,
                                    verdict.because()
                                ),
                            ),
                            (Format::Pdf, Some(verdict)) => Destination::Blocked(
                                "cannot be converted",
                                format!("This PDF cannot become an EPUB: {}.", verdict.because()),
                            ),
                            (Format::Pdf, None) => Destination::Blocked(
                                "not read yet",
                                "This PDF has not been read yet; refresh and try again.".into(),
                            ),
                            (format, _) => Destination::Blocked(
                                "reader shows only EPUB",
                                format!(
                                    "The reader stores a {} but never lists one, and a {} cannot become an EPUB.",
                                    format.label(),
                                    format.label()
                                ),
                            ),
                        };
                    }
                }
                Destination::Ready(if target == Place::CrossPoint {
                    "copy, optimized"
                } else {
                    "copy"
                })
            }
        }
    }
    pub fn removal(&self, book: &crate::books::Book, copy: &crate::books::Copy) -> Removal {
        if self.busy.is_some() {
            return Removal::Blocked("busy", "Wait for the current work to finish.".into());
        }
        if self.loading.is_some() {
            return Removal::Blocked(
                "scanning",
                "Wait for discovery to finish before deleting anything.".into(),
            );
        }
        if book.copies.len() < 2 {
            return Removal::Blocked(
                "only copy",
                format!(
                    "This is the only copy of {}; copy it somewhere else first.",
                    book.title
                ),
            );
        }
        if copy.locked {
            return Removal::Blocked(
                "Kobo library",
                "The Kobo database lists this book; remove it on the Kobo itself.".into(),
            );
        }
        if copy.sha.is_empty() {
            return Removal::Blocked(
                "unreadable",
                "This copy could not be read, so it cannot be identified.".into(),
            );
        }
        if copy.place == Place::CrossPoint && self.options.copy_to.is_none() {
            return Removal::Blocked(
                "needs the card",
                "Deleting over Wi-Fi is not supported; mount the reader's card.".into(),
            );
        }
        Removal::Ready
    }
    fn selected_entry(&self) -> Option<Entry> {
        self.filtered().get(self.selected).map(|e| (*e).clone())
    }
    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }
    fn quit(&mut self) -> Vec<Effect> {
        if self.busy.is_some() || self.loading.is_some() || self.configuring.is_some() {
            self.pending_quit = true;
            self.status = "Finishing current work before quitting…".into();
            vec![]
        } else {
            vec![Effect::Quit]
        }
    }
    pub fn update(&mut self, message: Message) -> Vec<Effect> {
        match message {
            Message::Corrected(id, result) if self.configuring == Some(id) => {
                self.configuring = None;
                match result {
                    // The library is read again, so the row shows what the book
                    // now says rather than what it said.
                    Ok(message) => {
                        self.correcting = None;
                        self.status = message;
                        if !self.pending_quit {
                            return self.update(Message::Refresh);
                        }
                    }
                    Err(e) => match &mut self.correcting {
                        Some(panel) => panel.message = e,
                        None => self.status = e,
                    },
                }
            }
            Message::Configured(id, result) if self.configuring == Some(id) => {
                self.configuring = None;
                match result {
                    Ok(super::settings::Outcome::Saved(defaults)) => {
                        self.options.browse = defaults
                            .browse
                            .unwrap_or_else(|| self.options.browse.clone());
                        self.options.output = defaults
                            .output
                            .unwrap_or_else(|| self.options.output.clone());
                        self.options.device = defaults.device;
                        self.options.send_to = defaults.reader;
                        self.options.copy_to = defaults.copy_to;
                        self.options.folder = defaults.folder.unwrap_or_else(|| "/".into());
                        self.options.first_run = false;
                        self.skipped = false;
                        self.settings = None;
                        // A changed location must not retain old, actionable copies.
                        self.entries.clear();
                        self.catalog = Snapshot::default();
                        self.marked.clear();
                        self.selected = 0;
                        self.scroll.set(0);
                        if !self.pending_quit {
                            return self.update(Message::Refresh);
                        }
                    }
                    result => {
                        if let Some(panel) = &mut self.settings {
                            match result {
                                Ok(super::settings::Outcome::Detected(devices)) => {
                                    panel.detected(devices)
                                }
                                Ok(super::settings::Outcome::Tested(text)) => panel.message = text,
                                Err(e) => panel.message = e,
                                _ => {}
                            }
                        }
                    }
                }
            }
            Message::Refresh if !self.pending_quit => {
                if self.loading.is_some() || self.busy.is_some() {
                    // Remembered rather than refused, so asking during a scan
                    // means "when you can" instead of "no".
                    self.pending_refresh = true;
                    self.status = "Will refresh when the current work finishes.".into();
                    return vec![];
                }
                self.pending_refresh = false;
                self.action.clear();
                self.copies = None;
                self.confirm = None;
                self.retaining = !self.entries.is_empty();
                self.pending_catalog = None;
                let id = self.id();
                self.loading = Some(id);
                self.status = "Discovering Local, Kobo and CrossPoint independently…".into();
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
                            b.preferred().map_or("EPUB", |c| c.format.label()),
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
                self.sort_entries();
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
                if let Some(effects) = self.deferred_refresh() {
                    return effects;
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
                if let Some(effects) = self.deferred_refresh() {
                    return effects;
                }
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
                if let Some(panel) = &mut self.correcting {
                    if self.configuring.is_some() {
                        return vec![];
                    }
                    match panel.input(key) {
                        super::edit::Action::Close => self.correcting = None,
                        super::edit::Action::Run(task) => {
                            panel.message = "Correcting…".into();
                            let id = self.id();
                            self.configuring = Some(id);
                            return vec![Effect::Correct { id, task }];
                        }
                        super::edit::Action::None => {}
                    }
                    return vec![];
                }
                if let Some(panel) = &mut self.settings {
                    if self.configuring.is_some() {
                        return vec![];
                    }
                    match panel.input(key) {
                        super::settings::Action::Close => {
                            let first_run = panel.first_run;
                            self.settings = None;
                            if first_run {
                                self.options.first_run = false;
                                self.skipped = true;
                                let effects = self.update(Message::Refresh);
                                self.status =
                                    "Setup skipped; nothing was saved. Press , to set it up later."
                                        .into();
                                return effects;
                            }
                        }
                        super::settings::Action::Run(task) => {
                            panel.message = match task.as_ref() {
                                super::settings::Task::Save { .. } => "Saving settings…",
                                super::settings::Task::Detect => "Looking for mounted Kobos…",
                                super::settings::Task::Test { .. } => "Testing reader connection…",
                            }
                            .into();
                            let id = self.id();
                            self.configuring = Some(id);
                            return vec![Effect::Configure { id, task: *task }];
                        }
                        super::settings::Action::None => {}
                    }
                    return vec![];
                }
                if self.history.is_some() {
                    if matches!(key.code, KeyCode::Esc | KeyCode::Char('h' | 'q')) {
                        self.history = None;
                    }
                    return vec![];
                }
                if self.help {
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => self.help = false,
                        KeyCode::Down | KeyCode::PageDown => {
                            self.help_scroll =
                                (self.help_scroll + 1).min(HELP.len().saturating_sub(1) as u16)
                        }
                        KeyCode::Up | KeyCode::PageUp => {
                            self.help_scroll = self.help_scroll.saturating_sub(1)
                        }
                        _ => {}
                    }
                    return vec![];
                }
                if let Some(selected) = &mut self.filter_menu {
                    let chosen = match key.code {
                        KeyCode::Down | KeyCode::Char('j') => {
                            *selected = (*selected + 1) % Filter::CYCLE.len();
                            None
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            *selected = (*selected + Filter::CYCLE.len() - 1) % Filter::CYCLE.len();
                            None
                        }
                        KeyCode::Enter => Some(*selected),
                        KeyCode::Char(c) if ('1'..='6').contains(&c) => {
                            Some(c as usize - '1' as usize)
                        }
                        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('f') => {
                            self.filter_menu = None;
                            None
                        }
                        _ => None,
                    };
                    if let Some(index) = chosen {
                        self.filter = Filter::CYCLE[index];
                        self.filter_menu = None;
                        self.selected = 0;
                        self.touched = false;
                        self.scroll.set(0);
                    }
                    return vec![];
                }
                if let Some(entry) = self.copies.clone() {
                    let Source::Book(book) = &entry.source else {
                        self.copies = None;
                        return vec![];
                    };
                    let book = self.current(book).clone();
                    if let Some((place, path)) = self.confirm.clone() {
                        // Only one key deletes, and only while the question is
                        // on screen; anything else is a refusal.
                        if key.code != KeyCode::Char('y') {
                            self.confirm = None;
                            self.status = "Nothing was deleted.".into();
                            return vec![];
                        }
                        let Some(copy) = book
                            .copies
                            .iter()
                            .find(|c| c.place == place && c.path == path)
                        else {
                            self.confirm = None;
                            self.status = "That copy is gone; refresh and look again.".into();
                            return vec![];
                        };
                        if let Removal::Blocked(_, reason) = self.removal(&book, copy) {
                            self.confirm = None;
                            self.status = reason;
                            return vec![];
                        }
                        let id = self.id();
                        self.busy = Some(id);
                        self.confirm = None;
                        self.copies = None;
                        self.status = format!("Deleting the {} copy…", place.label());
                        return vec![Effect::Remove {
                            id,
                            options: Box::new(self.options.clone()),
                            book: Box::new(book),
                            place,
                            path,
                        }];
                    }
                    let chosen = match key.code {
                        KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                            c.to_digit(10).map(|n| n as usize - 1)
                        }
                        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('d') => {
                            self.copies = None;
                            return vec![];
                        }
                        _ => None,
                    };
                    if let Some(copy) = chosen.and_then(|i| book.copies.get(i)) {
                        match self.removal(&book, copy) {
                            Removal::Blocked(_, reason) => self.status = reason,
                            Removal::Ready => {
                                self.confirm = Some((copy.place, copy.path.clone()));
                            }
                        }
                    }
                    return vec![];
                }
                if !self.action.is_empty() {
                    // While a destination waits for a yes, it is the only thing
                    // the dialog answers to.
                    let asked = self.ask.clone();
                    let target = match (&asked, key.code) {
                        (Some((target, _)), KeyCode::Char('y')) => Some(*target),
                        (Some(_), KeyCode::Esc | KeyCode::Char('q')) => {
                            self.ask = None;
                            return vec![];
                        }
                        (Some(_), _) => return vec![],
                        (None, KeyCode::Char('1')) => Some(Place::Local),
                        (None, KeyCode::Char('2')) => Some(Place::Kobo),
                        (None, KeyCode::Char('3')) => Some(Place::CrossPoint),
                        (None, KeyCode::Esc | KeyCode::Char('q')) => {
                            self.action.clear();
                            return vec![];
                        }
                        (None, _) => None,
                    };
                    let Some(target) = target else {
                        return vec![];
                    };
                    if asked.is_none() {
                        // One question covers the set, not one per book.
                        let question = self.action.iter().find_map(|entry| {
                            match self.destination(entry, target) {
                                Destination::Ask(_, reason) => Some(reason),
                                _ => None,
                            }
                        });
                        if let Some(reason) = question {
                            self.ask = Some((target, reason));
                            return vec![];
                        }
                    }
                    self.ask = None;
                    // Books that cannot be copied are left behind with their
                    // reason; a set is never refused as a whole for one of them.
                    let mut blocked = None;
                    let entries: Vec<Entry> = self
                        .action
                        .clone()
                        .into_iter()
                        .filter(|entry| match self.destination(entry, target) {
                            Destination::Ready(_) | Destination::Ask(_, _) => true,
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
                        // Clearing the line, as the settings panel does, rather
                        // than typing a literal u.
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            self.query.clear();
                            self.selected = 0;
                            self.touched = false;
                        }
                        // A control chord is not text: without this, Ctrl+U and
                        // its like arrive as the letter they are struck with.
                        KeyCode::Char(c)
                            if !key
                                .modifiers
                                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                                && !c.is_control() =>
                        {
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
                    KeyCode::Char('?') => {
                        self.help = true;
                        self.help_scroll = 0;
                    }
                    KeyCode::Char('e') => {
                        match self.selected_entry().map(|entry| entry.source.clone()) {
                            Some(Source::Book(book)) if self.busy.is_none() => {
                                let book = self.current(&book).clone();
                                match super::edit::Panel::new(&book) {
                                    Ok(panel) => self.correcting = Some(panel),
                                    Err(e) => self.status = format!("{e:#}"),
                                }
                            }
                            Some(_) => {
                                self.status = "Only a book's own metadata can be corrected.".into()
                            }
                            None => {}
                        }
                    }
                    KeyCode::Char('h') => {
                        // Read when asked for: a record that is not being
                        // looked at does not need to be held.
                        self.history = Some(crate::history::read(None, 200));
                    }
                    // Looking at settings costs nothing, so it is never
                    // refused: a scan of a reader over Wi-Fi takes long enough
                    // that being turned away reads as the key not working.
                    // A dot is what people reach for, dotfiles being what
                    // settings usually live in, so both open them.
                    KeyCode::Char(',' | '.') => self.open_settings(false),
                    KeyCode::Char('s') => {
                        let selected = self.selected_entry();
                        self.sort = self.sort.next();
                        self.sort_entries();
                        self.selected = selected
                            .and_then(|old| {
                                self.filtered().iter().position(|e| e.source == old.source)
                            })
                            .unwrap_or(0);
                        self.touched = true;
                        self.scroll.set(0);
                    }
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
                    KeyCode::Char('d') if self.busy.is_none() => {
                        self.copies = self
                            .selected_entry()
                            .filter(|e| matches!(e.source, Source::Book(_)));
                        if self.copies.is_none() {
                            self.status = "An import request has no copies to delete.".into();
                        }
                    }
                    KeyCode::Char('f') | KeyCode::Char('F') => {
                        self.filter_menu = Some(
                            Filter::CYCLE
                                .iter()
                                .position(|f| *f == self.filter)
                                .unwrap_or(0),
                        );
                    }
                    KeyCode::Enter if self.busy.is_none() => {
                        self.action = self.chosen();
                        // Asked once, when the dialog opens, rather than on
                        // every frame it is drawn for.
                        self.room = [Place::Local, Place::Kobo, Place::CrossPoint]
                            .map(|place| {
                                let path = match place {
                                    Place::Local => Some(self.options.output.as_path()),
                                    Place::Kobo => self.options.device.as_deref(),
                                    // Only a mounted card can be measured; a
                                    // reader over Wi-Fi has nothing to ask.
                                    Place::CrossPoint => self.options.copy_to.as_deref(),
                                };
                                (place, crate::books::room(path))
                            })
                            .to_vec();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        if self.pending_quit
            && self.busy.is_none()
            && self.loading.is_none()
            && self.configuring.is_none()
        {
            return vec![Effect::Quit];
        }
        vec![]
    }
}

pub(super) const HELP: &[&str] = &[
    "Navigate: arrows or j/k; Home/End; Page Up/Down",
    "Enter       Choose where to copy the selected or marked books",
    "Space       Mark or unmark the highlighted book",
    "a           Mark or clear all books shown by the current filter",
    "/           Search titles and authors; arrows still navigate",
    "            author: title: series: search one field on its own",
    "Esc/Enter   Leave search, keeping the query; Ctrl+U clears it",
    "f           Open filters; arrows + Enter or 1–6 select",
    "s           Sort by title, then author, then series (ascending)",
    "d           Inspect copies; 1–9 asks to delete one; y confirms",
    "h           What was copied lately, and whether it arrived",
    "e           Correct a book's title, author or series",
    "r           Refresh connected locations",
    ",           Settings: folders, Kobo detection and reader test",
    "Esc         Stop copying after this book; close a dialog; otherwise quit",
    "q / Ctrl+C  Quit after active work finishes",
    "",
    "L / K / C   Local / Kobo / CrossPoint",
    "● original   ◐ optimized device copy   · absent   ✗ unreadable",
    "⇩ ACSM request: choose Local to fulfill it (activation required)",
    "Marked books stay marked when hidden by a filter or search.",
];
