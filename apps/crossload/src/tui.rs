//! The terminal interface, on ambit's tui platform.
//!
//! The model (`model.rs`) stays pure: it changes state and describes effects.
//! This file turns those effects into platform tasks. Discovery is a chain of
//! tasks per location (list it, then read its books a few at a time); a copy
//! job is one serial task per book, so books going to one device go one after
//! the other.
mod edit;
mod model;
mod settings;
#[cfg(test)]
mod tests;
mod view;

use crate::books::{self, Budget, Item, Library, Listing, Place, Scan};
use crate::{cache, config, crosspoint, history};
use ambit_tui::{App, Buffer, Cmd, Event, Rect};
use model::{Effect, Message, Model};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Whether the view may use colour (`NO_COLOR` unset), as the wiring says.
pub(crate) static COLOR: AtomicBool = AtomicBool::new(true);

/// What the interface shows about where things are. Names only: the folders
/// and devices themselves come in as capabilities.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The settings file, for messages.
    pub config: String,
    pub home: Option<String>,
    pub first_run: bool,
    pub browse: String,
    pub output: String,
    pub device: Option<String>,
    pub send_to: Option<String>,
    pub copy_to: Option<String>,
    pub folder: String,
    /// The saved settings, carried through a save of the settings panel.
    pub defaults: config::Defaults,
    /// Kobos the wiring found mounted at launch, for the settings panel.
    pub kobos: Vec<String>,
    /// Which configured places the wiring could open at launch.
    pub opened: Opened,
    pub color: bool,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Opened {
    pub books: bool,
    pub import: bool,
    pub kobo: Option<bool>,
    pub card: Option<bool>,
}
#[derive(Clone, Debug, PartialEq)]
enum Source {
    /// An ACSM request, by its local path.
    Local(String),
    Book(Box<crate::books::Book>),
}
#[derive(Clone, Debug)]
struct Entry {
    title: String,
    author: String,
    kind: &'static str,
    source: Source,
    /// Title and author folded once, so searching does not rebuild a string
    /// per book on every keystroke and every frame.
    search: String,
    title_key: String,
    author_key: String,
    /// The series and position, folded into one key so a series sorts in its
    /// own order. A book in no series sorts after every book in one.
    series_key: String,
}
impl Entry {
    fn new(title: String, author: String, kind: &'static str, source: Source) -> Self {
        // A series is part of what a book is called, so a plain search finds it
        // too: correcting a title that carried the series would otherwise take
        // the series out of reach of the search box.
        let series_name = match &source {
            Source::Book(book) => book.series.clone().unwrap_or_default(),
            _ => String::new(),
        };
        let search = format!("{title} {author} {series_name}").to_lowercase();
        let series_key = match &source {
            Source::Book(book) => match (&book.series, book.series_index) {
                // Pad the position so 2 sorts before 10, and keep a half
                // number such as 1.5 between them.
                (Some(series), index) => format!(
                    "0{}\u{1}{:09.3}",
                    series.to_lowercase(),
                    index.unwrap_or(0.0)
                ),
                (None, _) => "1".to_owned(),
            },
            _ => "1".to_owned(),
        };
        Self {
            title_key: title.to_lowercase(),
            author_key: author.to_lowercase(),
            series_key,
            title,
            author,
            kind,
            source,
            search,
        }
    }
}

/// How many books of one location are read at once. Books are read whole, so
/// the budget in `books::Budget` bounds the memory as well.
fn readers(place: Place) -> usize {
    match place {
        Place::Local => 4,
        // One device, one database, one Wi-Fi connection.
        Place::Kobo | Place::CrossPoint => 1,
    }
}
const PLACES: [Place; 3] = [Place::Local, Place::Kobo, Place::CrossPoint];
/// How often a partial catalog is shown while locations are working.
const INTERVAL: Duration = Duration::from_millis(200);
const TICK: Duration = Duration::from_millis(150);

pub(crate) enum Msg {
    Model(Message),
    Tick,
    Indexed(u64, Arc<cache::Index>),
    Listed(u64, Place, Result<Listing, String>),
    Read(u64, Place, Box<books::Book>),
    Saved(u64),
    /// One book of a copy job is done.
    Copied(u64, Result<String, String>),
}

/// A discovery in progress.
struct Scanning {
    id: u64,
    scan: Scan,
    index: Option<Arc<cache::Index>>,
    budget: Arc<Budget>,
    queues: [VecDeque<Item>; 3],
    running: [usize; 3],
    done: [bool; 3],
    shown: Option<Duration>,
}
/// A copy job in progress.
struct Job {
    id: u64,
    target: Place,
    pending: VecDeque<Entry>,
    cancel: Arc<AtomicBool>,
    total: usize,
    copied: usize,
    failures: Vec<String>,
    last: String,
}

pub(crate) struct Shell {
    model: Model,
    first: Vec<Effect>,
    library: Arc<Library>,
    store: Option<config::Store>,
    link: crosspoint::Link,
    relaunch: Arc<AtomicBool>,
    scanning: Option<Scanning>,
    job: Option<Job>,
    ticking: bool,
}

fn index(place: Place) -> usize {
    PLACES.iter().position(|p| *p == place).expect("a place")
}

impl Shell {
    pub(crate) fn new(
        options: Options,
        library: Library,
        store: Option<config::Store>,
        link: crosspoint::Link,
        relaunch: Arc<AtomicBool>,
    ) -> Self {
        COLOR.store(options.color, Ordering::Relaxed);
        let mut model = Model::new(options);
        let first = if model.options.first_run {
            model.open_settings(true);
            vec![]
        } else {
            model.update(Message::Refresh)
        };
        Self {
            model,
            first,
            library: Arc::new(library),
            store,
            link,
            relaunch,
            scanning: None,
            job: None,
            ticking: false,
        }
    }

    fn update_model(&mut self, message: Message) -> Cmd<Msg> {
        let effects = self.model.update(message);
        self.run(effects)
    }

    /// Keeps the spinner turning while something runs.
    fn tick(&mut self) -> Cmd<Msg> {
        let working = self.model.busy.is_some() || self.model.loading.is_some();
        if working && !self.ticking {
            self.ticking = true;
            return Cmd::After(TICK, Msg::Tick);
        }
        Cmd::None
    }

    fn run(&mut self, effects: Vec<Effect>) -> Cmd<Msg> {
        let mut cmds: Vec<Cmd<Msg>> = effects.into_iter().map(|e| self.effect(e)).collect();
        cmds.push(self.tick());
        Cmd::Batch(cmds)
    }

    fn effect(&mut self, effect: Effect) -> Cmd<Msg> {
        let library = self.library.clone();
        match effect {
            Effect::Quit => Cmd::Quit,
            Effect::Relaunch => {
                self.relaunch.store(true, Ordering::Relaxed);
                Cmd::Quit
            }
            Effect::Load { id } => {
                self.scanning = Some(Scanning {
                    id,
                    scan: Scan::new(),
                    index: None,
                    budget: Arc::new(Budget::new()),
                    queues: Default::default(),
                    running: [0; 3],
                    done: [false; 3],
                    shown: None,
                });
                let store = library.cache.clone();
                let clock = library.clock.clone();
                Cmd::task(move || Msg::Indexed(id, Arc::new(cache::Index::open(store, clock))))
            }
            Effect::Work {
                id,
                entries,
                target,
                cancel,
            } => {
                self.job = Some(Job {
                    id,
                    target,
                    total: entries.len(),
                    pending: entries.into(),
                    cancel,
                    copied: 0,
                    failures: vec![],
                    last: String::new(),
                });
                self.next_copy()
            }
            Effect::Remove {
                id,
                book,
                place,
                path,
            } => Cmd::serial(move || {
                let result = books::remove(&library, &book, place, &path, &|_| {})
                    .map(|message| format!("{message}. Press r to refresh."))
                    .map_err(|e| format!("{e:#}"));
                Msg::Model(Message::Finished(id, result))
            }),
            Effect::Correct { id, task } => Cmd::task(move || {
                let result = edit::perform(&library, task).map_err(|e| format!("{e:#}"));
                Msg::Model(Message::Corrected(id, result))
            }),
            Effect::Configure { id, task } => {
                let store = self.store.clone();
                let kobos = self.model.options.kobos.clone();
                let link = self.link.clone();
                Cmd::task(move || {
                    let result = settings::perform(
                        task,
                        settings::Context {
                            store: store.as_ref(),
                            kobos: &kobos,
                            link: Some(&link),
                        },
                    )
                    .map_err(|e| format!("{e:#}"));
                    Msg::Model(Message::Configured(id, result))
                })
            }
            Effect::History => Cmd::task(move || {
                Msg::Model(Message::History(history::read(
                    library.history.as_ref(),
                    200,
                )))
            }),
            Effect::Room => Cmd::task(move || {
                let room = PLACES
                    .iter()
                    .map(|place| (*place, books::room(&library, *place)))
                    .collect();
                Msg::Model(Message::Room(room))
            }),
        }
    }

    /// Start the next book of the copy job, or report how it went.
    fn next_copy(&mut self) -> Cmd<Msg> {
        let Some(job) = &mut self.job else {
            return Cmd::None;
        };
        let (id, total, target) = (job.id, job.total, job.target);
        let index = total - job.pending.len();
        let stopped = job.cancel.load(Ordering::Relaxed) && !job.pending.is_empty();
        let next = if stopped {
            None
        } else {
            job.pending.pop_front()
        };
        let Some(entry) = next else {
            let job = self.job.take().expect("a job");
            let result = summary(&job, stopped);
            return self.update_model(Message::Finished(id, result));
        };
        let mut cmds = vec![self.update_model(Message::Step(id, index, total))];
        let text = if total > 1 {
            format!("{}…", entry.title)
        } else {
            match &entry.source {
                Source::Local(_) => "Fulfilling ACSM and importing EPUB…".to_owned(),
                Source::Book(_) => format!("Copying to {} and verifying…", target.label()),
            }
        };
        cmds.push(self.update_model(Message::Progress(id, text)));
        let library = self.library.clone();
        cmds.push(Cmd::serial(move || {
            let notes = std::sync::Mutex::new(Vec::new());
            let progress = |text: &str| {
                if text.starts_with("Note:") {
                    if let Ok(mut notes) = notes.lock() {
                        notes.push(text.to_owned());
                    }
                }
            };
            let result = match &entry.source {
                Source::Book(book) => books::transfer(&library, book, target, &progress),
                Source::Local(path) => crate::acsm::fulfil(&library, path, &progress),
            };
            let notes = notes.into_inner().unwrap_or_default();
            let result = result
                .map(|message| {
                    std::iter::once(message)
                        .chain(notes)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .map_err(|e| {
                    if total > 1 {
                        format!("{}: {e:#}", entry.title)
                    } else {
                        format!("{e:#}")
                    }
                });
            Msg::Copied(id, result)
        }));
        Cmd::Batch(cmds)
    }

    /// Start reading more books, as many as each location allows.
    fn read_more(&mut self) -> Cmd<Msg> {
        let Some(scanning) = &mut self.scanning else {
            return Cmd::None;
        };
        let Some(index) = scanning.index.clone() else {
            return Cmd::None;
        };
        let mut cmds = vec![];
        for place in PLACES {
            let i = self::index(place);
            while scanning.running[i] < readers(place) {
                let Some(item) = scanning.queues[i].pop_front() else {
                    break;
                };
                scanning.running[i] += 1;
                let (library, index, budget, id) = (
                    self.library.clone(),
                    index.clone(),
                    scanning.budget.clone(),
                    scanning.id,
                );
                cmds.push(Cmd::task(move || {
                    let _claim = budget.claim(item.size());
                    let book = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        library.read(place, item, &index)
                    }))
                    // A book whose reading panicked is left out rather than
                    // stopping the scan; it has no copy to show.
                    .unwrap_or_else(|_| books::Book {
                        title: String::new(),
                        author: String::new(),
                        series: None,
                        series_index: None,
                        copies: vec![],
                    });
                    Msg::Read(id, place, Box::new(book))
                }));
            }
        }
        Cmd::Batch(cmds)
    }

    /// Show the catalog so far: at once for the first books and each finished
    /// location, otherwise at a readable rate.
    fn show(&mut self, force: bool) -> Cmd<Msg> {
        let now = self.library.clock.elapsed();
        let Some(scanning) = &mut self.scanning else {
            return Cmd::None;
        };
        let due = match scanning.shown {
            None => !scanning.scan.is_empty(),
            Some(at) => now.saturating_sub(at) >= INTERVAL,
        };
        if !(force || due) {
            return Cmd::None;
        }
        scanning.shown = Some(now);
        let (id, snapshot) = (scanning.id, scanning.scan.snapshot());
        self.update_model(Message::Catalog(id, snapshot))
    }

    /// A location is done when its listing came in and every book it found
    /// has been read.
    fn settle(&mut self, place: Place) -> Cmd<Msg> {
        let Some(scanning) = &mut self.scanning else {
            return Cmd::None;
        };
        let i = index(place);
        if scanning.done[i] || scanning.running[i] > 0 || !scanning.queues[i].is_empty() {
            return Cmd::None;
        }
        scanning.done[i] = true;
        scanning.scan.finished(place, Ok(()));
        self.finish_if_done()
    }

    fn finish_if_done(&mut self) -> Cmd<Msg> {
        let shown = self.show(true);
        let Some(scanning) = &self.scanning else {
            return shown;
        };
        if !scanning.done.iter().all(|d| *d) {
            return shown;
        }
        let scanning = self.scanning.take().expect("scanning");
        let id = scanning.id;
        let index = scanning.index;
        Cmd::Batch(vec![
            shown,
            Cmd::task(move || {
                if let Some(index) = index {
                    index.save();
                }
                Msg::Saved(id)
            }),
        ])
    }
}

/// How a copy job went, as the status line says it. One failure does not
/// abandon the rest; the summary names what went wrong.
fn summary(job: &Job, stopped: bool) -> Result<String, String> {
    if job.total == 1 && job.failures.is_empty() {
        return Ok(format!("{} Press r to refresh.", job.last));
    }
    if job.total == 1 {
        return Err(job.failures[0].clone());
    }
    let mut summary = format!(
        "Copied {} of {} to {}.",
        job.copied,
        books::books(job.total),
        job.target.label()
    );
    if stopped {
        summary.push_str(" Stopped on request.");
    }
    if !job.failures.is_empty() {
        summary.push_str(&format!(
            " {} failed: {}{}",
            job.failures.len(),
            job.failures
                .iter()
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
                .join("; "),
            if job.failures.len() > 2 { "; …" } else { "" }
        ));
    }
    summary.push_str(" Press r to refresh.");
    Ok(summary)
}

impl App for Shell {
    type Msg = Msg;

    fn start(&mut self) -> Cmd<Msg> {
        let first = std::mem::take(&mut self.first);
        self.run(first)
    }

    fn update(&mut self, event: Event<Msg>) -> Cmd<Msg> {
        match event {
            Event::Key(key) => self.update_model(Message::Input(key)),
            Event::Paste(_) | Event::Resize(..) => Cmd::None,
            Event::Msg(Msg::Model(message)) => self.update_model(message),
            Event::Msg(Msg::Tick) => {
                self.ticking = false;
                self.update_model(Message::Tick)
            }
            Event::Msg(Msg::Indexed(id, index)) => {
                let Some(scanning) = self.scanning.as_mut().filter(|s| s.id == id) else {
                    return Cmd::None;
                };
                scanning.index = Some(index.clone());
                Cmd::Batch(
                    PLACES
                        .into_iter()
                        .map(|place| {
                            let (library, index) = (self.library.clone(), index.clone());
                            Cmd::task(move || {
                                let listing =
                                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                        library.list(place, &index)
                                    }))
                                    .unwrap_or_else(|_| {
                                        Err(anyhow::anyhow!("Location worker stopped"))
                                    })
                                    .map_err(|e| format!("{e:#}"));
                                Msg::Listed(id, place, listing)
                            })
                        })
                        .collect(),
                )
            }
            Event::Msg(Msg::Listed(id, place, listing)) => {
                let Some(scanning) = self.scanning.as_mut().filter(|s| s.id == id) else {
                    return Cmd::None;
                };
                match listing {
                    Ok(listing) => {
                        let todo = scanning.scan.listed(place, listing);
                        scanning.queues[index(place)].extend(todo);
                        let more = self.read_more();
                        let shown = self.show(false);
                        Cmd::Batch(vec![more, shown, self.settle(place)])
                    }
                    Err(e) => {
                        scanning.done[index(place)] = true;
                        scanning.scan.finished(place, Err(e));
                        self.finish_if_done()
                    }
                }
            }
            Event::Msg(Msg::Read(id, place, book)) => {
                let Some(scanning) = self.scanning.as_mut().filter(|s| s.id == id) else {
                    return Cmd::None;
                };
                scanning.running[index(place)] -= 1;
                if !book.copies.is_empty() {
                    scanning.scan.book(place, *book);
                }
                let more = self.read_more();
                let shown = self.show(false);
                Cmd::Batch(vec![more, shown, self.settle(place)])
            }
            Event::Msg(Msg::Saved(id)) => self.update_model(Message::CatalogFinished(id, Ok(()))),
            Event::Msg(Msg::Copied(id, result)) => {
                let Some(job) = self.job.as_mut().filter(|job| job.id == id) else {
                    return Cmd::None;
                };
                match result {
                    Ok(message) => {
                        job.copied += 1;
                        job.last = message;
                    }
                    Err(e) => job.failures.push(e),
                }
                self.next_copy()
            }
        }
    }

    fn view(&self, area: Rect, buf: &mut Buffer) {
        view::draw(&self.model, &mut view::Frame::new(area, buf));
    }
}
