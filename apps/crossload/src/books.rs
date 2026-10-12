//! A content-based library across local storage, Kobo and CrossPoint.
use crate::{cache, copy, epub, files, format, format::Format, history, inventory, kobo, prepare};
use ambit_caps::Clock;
use ambit_caps_dir::{DirWrite, RelPath};
use anyhow::{ensure, Context, Result};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum Place {
    Local,
    Kobo,
    CrossPoint,
}
impl Place {
    pub fn label(self) -> &'static str {
        match self {
            Self::Local => "Local",
            Self::Kobo => "Kobo",
            Self::CrossPoint => "CrossPoint",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Copy {
    pub place: Place,
    pub format: Format,
    /// For a PDF, what converting it to EPUB would be worth.
    pub verdict: Option<crate::pdf::Verdict>,
    pub path: String,
    pub size: u64,
    pub sha: String,
    pub resources: Option<String>,
    pub optimized: bool,
    /// A Kobo store book: the device database lists it, so its file is not ours
    /// to remove. Sideloaded books and everything elsewhere are.
    pub locked: bool,
    /// How this copy is recognized again; an implementation detail, not output.
    #[serde(skip)]
    variants: Vec<(String, Option<String>)>,
}
#[cfg(test)]
/// A copy for interface tests, which cannot see how matching is stored.
pub(crate) fn copy(place: Place, path: &str, size: u64, locked: bool) -> Copy {
    Copy {
        place,
        format: Format::of(path).unwrap_or_default(),
        verdict: None,
        path: path.to_owned(),
        size,
        sha: format!("sha-of-{path}"),
        resources: None,
        optimized: false,
        locked,
        variants: vec![],
    }
}
// Not Eq: a series index is a number that may be 1.5, and books are compared
// by their copies rather than looked up in a set.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Book {
    pub title: String,
    pub author: String,
    /// The series this book belongs to, and its place in it, when it says.
    pub series: Option<String>,
    pub series_index: Option<f32>,
    pub copies: Vec<Copy>,
}
impl Book {
    pub fn has(&self, place: Place) -> bool {
        self.copies.iter().any(|c| c.place == place)
    }
    pub fn preferred(&self) -> Option<&Copy> {
        self.copies.iter().min_by_key(|c| {
            (
                c.optimized,
                match c.place {
                    Place::Local => 0,
                    Place::Kobo => 1,
                    Place::CrossPoint => 2,
                },
            )
        })
    }
}
/// A folder on this computer, and how the user knows it.
#[derive(Clone)]
pub struct Folder {
    /// Books are read from it and added to it; nothing in it is changed or
    /// removed.
    pub dir: Arc<dyn DirWrite>,
    pub display: String,
}
/// Choices that shape what is read and made, but grant nothing.
#[derive(Clone, Debug, Default)]
pub struct Settings {
    pub show_previews: bool,
    pub serial: Option<String>,
    /// The screen to make device copies for, or None to leave images alone.
    pub optimize: Option<crate::profile::Profile>,
    pub organized: bool,
    /// Send books to the Kobo as kepubs rather than plain EPUBs.
    pub kepub: bool,
}
/// Every place books are, with the capabilities to reach each one.
#[derive(Clone)]
pub struct Library {
    pub settings: Settings,
    /// The books folder: where the user's own books are.
    pub books: Folder,
    /// The import folder: where imported, converted and copied-back books go.
    pub import: Folder,
    pub kobo: Option<kobo::Device>,
    /// The reader: its mounted card when there is one, otherwise over Wi-Fi.
    pub reader: Option<inventory::Destination>,
    pub adept: Arc<dyn crossload_adobe::Adept>,
    pub cache: Option<cache::Store>,
    pub history: Option<history::Record>,
    pub clock: Arc<dyn Clock>,
    /// Why a place has nothing to offer, when it was asked for and could not
    /// be had: not mounted, not configured.
    pub unavailable: Vec<(Place, String)>,
}
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub books: Vec<Book>,
    pub status: Vec<(Place, String)>,
    /// ACSM requests waiting to be fulfilled, as local paths.
    pub acsm: Vec<String>,
}
impl Snapshot {
    pub fn ready(&self, place: Place) -> bool {
        self.status
            .iter()
            .any(|(p, s)| *p == place && s.starts_with("Ready"))
    }
}
fn book(entry: cache::Source, place: Place, path: String) -> Book {
    Book {
        title: entry.title,
        author: entry.author,
        series: entry.series,
        series_index: entry.series_index,
        copies: vec![Copy {
            place,
            format: entry.format,
            verdict: entry.verdict.clone(),
            path,
            size: entry.size,
            sha: entry.sha,
            resources: entry.resources,
            optimized: entry.optimized,
            locked: entry.locked,
            variants: entry.variants,
        }],
    }
}
/// A location's stored identity for a source it has just enumerated and found
/// unchanged. Nothing here invents a book: the caller has already seen it.
fn stored(index: &cache::Index, key: Option<&String>, place: Place, source: &str) -> Option<Book> {
    Some(book(index.source(key?)?, place, source.to_owned()))
}
/// `key` is the caller's evidence that this source is unchanged; `None` means
/// this location cannot prove it, and the result is not stored.
#[allow(clippy::too_many_arguments)]
fn candidate(
    data: Vec<u8>,
    place: Place,
    format: Format,
    source: String,
    settings: &Settings,
    index: &cache::Index,
    key: Option<String>,
    locked: bool,
) -> Result<Book> {
    ensure!(
        data.len() as u64 <= epub::MAX_BOOK_BYTES,
        "Book exceeds 128 MiB"
    );
    format::validate(format, &data)?;
    let (title, author, series, series_index) = if format.rewritten() {
        let metadata = epub::metadata_from(&data)?.context("Missing EPUB metadata")?;
        (
            metadata.title.unwrap_or_else(|| "Untitled".into()),
            metadata.author,
            metadata.series,
            metadata.series_index,
        )
    } else {
        // A PDF says who wrote it and what it is called, when it has been
        // filled in at all. A CBZ says nothing, and neither does an empty
        // field, so what the file is called is the honest answer instead.
        let info = (format == Format::Pdf)
            .then(|| crate::pdf::info(&data).ok())
            .flatten()
            .unwrap_or_default();
        (
            info.title.unwrap_or_else(|| format::name_title(&source)),
            info.author.unwrap_or_default(),
            None,
            None,
        )
    };
    let id = inventory::identity(&data);
    // Only an EPUB is ever rebuilt, so only an EPUB can be a device copy: a
    // PDF on the reader is the same bytes that left here.
    let optimized = format.rewritten()
        && (place == Place::CrossPoint
            || zip::ZipArchive::new(std::io::Cursor::new(&data))?
                .by_name("META-INF/xteink-device-profile.txt")
                .is_ok());
    let mut variants = vec![(id.sha256.clone(), id.resources.clone())];
    if format.rewritten() && place != Place::CrossPoint {
        // Identical bytes always optimize to the same copy, wherever they came
        // from, so this hit also spares Kobo books the image re-encoding.
        let key = cache::variant_key(&id.sha256, settings.optimize, settings.organized);
        let variant = match index.variant(&key) {
            Some(variant) => variant,
            None => {
                let name = format!("book.{}", format.extension());
                let prepared =
                    prepare::prepare(data.clone(), &name, settings.optimize, settings.organized)?;
                let identity = inventory::identity(&prepared.data);
                let variant = cache::Variant {
                    sha: identity.sha256,
                    resources: identity.resources,
                };
                index.put_variant(key, variant.clone());
                variant
            }
        };
        variants.push((variant.sha, variant.resources));
    }
    // Judging a PDF costs only CPU on bytes already in hand, and the answer is
    // cached with everything else about the file.
    let verdict = (format == Format::Pdf)
        .then(|| crate::pdf::inspect(&data).map(|report| report.verdict).ok())
        .flatten();
    let entry = cache::Source {
        title,
        author,
        series,
        series_index,
        format,
        verdict,
        size: data.len() as u64,
        sha: id.sha256,
        resources: id.resources,
        optimized,
        locked,
        variants,
    };
    if let Some(key) = key {
        index.put_source(key, entry.clone());
    }
    Ok(book(entry, place, source))
}
fn unreadable(place: Place, format: Format, path: String, title: String, reason: String) -> Book {
    Book {
        title,
        author: format!("Unreadable: {reason}"),
        series: None,
        series_index: None,
        copies: vec![Copy {
            place,
            format,
            verdict: None,
            path,
            size: 0,
            sha: String::new(),
            resources: None,
            optimized: format.rewritten() && place == Place::CrossPoint,
            locked: false,
            variants: vec![],
        }],
    }
}
fn matches(a: &Copy, b: &Copy) -> bool {
    a.variants.iter().any(|(sha, resources)| {
        b.variants
            .iter()
            .any(|(s, r)| sha == s || resources.as_ref().is_some_and(|v| Some(v) == r.as_ref()))
    })
}
pub fn merge(books: &mut Vec<Book>, book: Book) {
    let mut combined = book;
    let mut i = 0;
    while i < books.len() {
        if books[i]
            .copies
            .iter()
            .any(|a| combined.copies.iter().any(|b| matches(a, b)))
        {
            let old = books.remove(i);
            for copy in old.copies {
                if !combined
                    .copies
                    .iter()
                    .any(|c| c.place == copy.place && c.path == copy.path)
                {
                    combined.copies.push(copy);
                }
            }
            i = 0;
        } else {
            i += 1;
        }
    }
    books.push(combined);
}
/// Reading a book holds its bytes, its prepared copy and that copy's output at
/// once, so several large books in flight can cost far more than the books
/// themselves. Workers claim from a shared budget before reading and release it
/// afterwards; a book larger than the whole budget waits for exclusive use of
/// it rather than deadlocking.
pub struct Budget {
    free: std::sync::Mutex<u64>,
    released: std::sync::Condvar,
}
pub struct Claim<'a> {
    budget: &'a Budget,
    amount: u64,
}
impl Budget {
    /// Enough for several ordinary books at once, and a ceiling a single very
    /// large one cannot exceed.
    const TOTAL: u64 = 384 * 1024 * 1024;
    pub fn new() -> Self {
        Self {
            free: std::sync::Mutex::new(Self::TOTAL),
            released: std::sync::Condvar::new(),
        }
    }
    pub fn claim(&self, size: u64) -> Claim<'_> {
        let amount = size.saturating_mul(3).clamp(1, Self::TOTAL);
        let mut free = self.free.lock().unwrap_or_else(|e| e.into_inner());
        while *free < amount {
            free = self.released.wait(free).unwrap_or_else(|e| e.into_inner());
        }
        *free -= amount;
        Claim {
            budget: self,
            amount,
        }
    }
}
impl Default for Budget {
    fn default() -> Self {
        Self::new()
    }
}
impl Drop for Claim<'_> {
    fn drop(&mut self) {
        if let Ok(mut free) = self.budget.free.lock() {
            *free += self.amount;
            self.budget.released.notify_all();
        }
    }
}
/// "1 book", "4 books": counts that read as English wherever they are shown.
pub fn books(count: usize) -> String {
    format!("{count} book{}", if count == 1 { "" } else { "s" })
}
/// Snapshots are ordered when they are emitted, not on every insertion.
pub fn sort(books: &mut [Book]) {
    books.sort_by(|a, b| {
        a.title
            .to_lowercase()
            .cmp(&b.title.to_lowercase())
            .then(a.author.cmp(&b.author))
    });
}
/// Which local folder a file is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Root {
    Books,
    Import,
}
/// One thing a location found and has yet to read.
#[derive(Clone, Debug)]
pub enum Item {
    Local {
        root: Root,
        path: RelPath,
        /// How the copy is named: the folder as the user knows it, then the
        /// path inside it.
        source: String,
        key: Option<String>,
        size: u64,
    },
    Kobo {
        book: kobo::Book,
        key: Option<String>,
    },
    Reader {
        file: inventory::FileEntry,
        key: Option<String>,
    },
}
impl Item {
    /// What reading it costs in memory, roughly.
    pub fn size(&self) -> u64 {
        match self {
            Self::Local { size, .. } => *size,
            Self::Kobo { .. } => epub::MAX_BOOK_BYTES / 8,
            Self::Reader { file, .. } => file.size,
        }
    }
}
/// What listing one location found: the books it already knows, and the
/// ones it still has to read.
#[derive(Debug, Default)]
pub struct Listing {
    /// From the identity cache.
    pub known: Vec<Book>,
    /// Kobo previews: known without reading, and not a failed read.
    pub previews: Vec<Book>,
    pub todo: Vec<Item>,
    pub acsm: Vec<String>,
    /// Folders that could not be listed.
    pub unreadable: usize,
}
/// Whether two copies of an ACSM are the same request.
fn spent(dir: &dyn DirWrite, path: &RelPath, data: &[u8]) -> bool {
    let Some((parent, _)) = path.split_last() else {
        return false;
    };
    let Ok(archive) = parent.join("archive") else {
        return false;
    };
    dir.list(&archive).is_ok_and(|entries| {
        entries.iter().filter(|e| !e.is_dir).any(|e| {
            archive
                .join(&e.name)
                .ok()
                .and_then(|p| files::read(dir, &p, 8 << 20).ok())
                .is_some_and(|archived| archived == data)
        })
    })
}
impl Library {
    pub fn folder(&self, root: Root) -> &Folder {
        match root {
            Root::Books => &self.books,
            Root::Import => &self.import,
        }
    }
    /// The folder and path a local copy's name points at.
    pub fn locate(&self, source: &str) -> Result<(Root, RelPath)> {
        // The import folder may sit inside the books folder, so the longer
        // match wins.
        let mut roots = [(Root::Books, &self.books), (Root::Import, &self.import)];
        roots.sort_by_key(|(_, f)| std::cmp::Reverse(f.display.len()));
        for (root, folder) in roots {
            if let Some(rest) = source
                .strip_prefix(folder.display.trim_end_matches('/'))
                .and_then(|rest| rest.strip_prefix('/'))
            {
                return Ok((root, files::rel(rest)?));
            }
        }
        anyhow::bail!("{source} is not in the books or import folder")
    }
    fn source(&self, root: Root, path: &RelPath) -> String {
        format!("{}/{path}", self.folder(root).display.trim_end_matches('/'))
    }
    pub fn card(&self) -> Option<&inventory::Card> {
        match &self.reader {
            Some(inventory::Destination::Card(card)) => Some(card),
            _ => None,
        }
    }
    fn missing(&self, place: Place) -> anyhow::Error {
        anyhow::anyhow!(
            "{}",
            self.unavailable
                .iter()
                .find(|(p, _)| *p == place)
                .map_or("Not configured", |(_, why)| why.as_str())
        )
    }
    /// Walk a location and say what is there. Cheap: books are only read when
    /// the cache does not know them, and that is left to [`Library::read`].
    pub fn list(&self, place: Place, index: &cache::Index) -> Result<Listing> {
        let mut listing = Listing::default();
        let settings = &self.settings;
        match place {
            Place::Local => {
                let mut roots = vec![Root::Books];
                // An import folder inside the books folder is walked with it.
                let inside = format!("{}/", self.books.display.trim_end_matches('/'));
                if self.import.display != self.books.display
                    && !self.import.display.starts_with(&inside)
                {
                    roots.push(Root::Import);
                }
                // Walking a tree is cheap; reading and hashing books is not.
                let mut visited = 0;
                let mut any = false;
                for root in roots {
                    let dir = &*self.folder(root).dir;
                    if !dir.metadata(&RelPath::root()).is_ok_and(|m| m.is_dir) {
                        continue;
                    }
                    any = true;
                    let mut pending = vec![(RelPath::root(), 0)];
                    while let Some((folder, depth)) = pending.pop() {
                        ensure!(depth <= 32, "Local tree exceeds 32 levels");
                        let Ok(items) = dir.list(&folder) else {
                            listing.unreadable += 1;
                            continue;
                        };
                        for item in items {
                            visited += 1;
                            ensure!(
                                visited <= 20000,
                                "Local tree exceeds 20000 entries; choose a narrower --browse folder"
                            );
                            if item.name.starts_with('.') {
                                continue;
                            }
                            let path = folder.join(&item.name)?;
                            if item.is_dir {
                                pending.push((path, depth + 1));
                                continue;
                            }
                            let ext = item
                                .name
                                .rsplit_once('.')
                                .map(|(_, e)| e.to_ascii_lowercase())
                                .unwrap_or_default();
                            if matches!(ext.as_str(), "acsm" | "ascm") {
                                // A fulfilled request is archived beside itself;
                                // it is spent, not pending.
                                let archived =
                                    folder.parts().last().is_some_and(|p| p == "archive");
                                let pending = !archived
                                    && files::read(dir, &path, 8 << 20)
                                        .map(|data| !spent(dir, &path, &data))
                                        .unwrap_or(true);
                                if pending {
                                    listing.acsm.push(self.source(root, &path));
                                }
                                continue;
                            }
                            if !Format::ALL.iter().any(|f| f.extension() == ext) {
                                continue;
                            }
                            let source = self.source(root, &path);
                            let meta = match dir.metadata(&path) {
                                // A link that leads out of the folder is not
                                // part of it, and is skipped.
                                Err(e) if e.kind == ambit_caps_dir::DirErrorKind::Denied => {
                                    continue
                                }
                                meta => meta.ok(),
                            };
                            let key = meta.as_ref().and_then(cache::file_fingerprint).map(
                                |fingerprint| {
                                    cache::source_key(
                                        place.label(),
                                        &source,
                                        &fingerprint,
                                        settings.optimize,
                                        settings.organized,
                                    )
                                },
                            );
                            match stored(index, key.as_ref(), place, &source) {
                                Some(book) => listing.known.push(book),
                                None => listing.todo.push(Item::Local {
                                    root,
                                    path,
                                    source,
                                    key,
                                    size: meta.map_or(0, |m| m.len),
                                }),
                            }
                        }
                    }
                }
                ensure!(any, "Local folders unavailable");
            }
            Place::Kobo => {
                let device = self.kobo.as_ref().ok_or_else(|| self.missing(place))?;
                for book in device
                    .books()?
                    .into_iter()
                    .filter(|b| settings.show_previews || !b.preview)
                {
                    if book.preview {
                        // A preview is a known state, not a failed read.
                        listing.previews.push(unreadable(
                            place,
                            Format::Epub,
                            book.id,
                            format!("[Preview] {}", book.title),
                            "Download the full book on Kobo first".into(),
                        ));
                        continue;
                    }
                    // The decrypted copy follows from the device file, the
                    // serial and this library, so an unchanged file needs no
                    // import.
                    let scope = format!(
                        "{}\u{1}{}\u{1}{}",
                        place.label(),
                        device.display,
                        settings.serial.as_deref().unwrap_or_default()
                    );
                    let key = device
                        .source_path(&book)
                        .ok()
                        .and_then(|path| device.dir.metadata(&path).ok())
                        .as_ref()
                        .and_then(cache::file_fingerprint)
                        .map(|fingerprint| {
                            cache::source_key(
                                &scope,
                                &book.id,
                                &fingerprint,
                                settings.optimize,
                                settings.organized,
                            )
                        });
                    match stored(index, key.as_ref(), place, &book.id) {
                        Some(found) => listing.known.push(found),
                        None => listing.todo.push(Item::Kobo { book, key }),
                    }
                }
            }
            Place::CrossPoint => {
                let destination = self.reader.as_ref().ok_or_else(|| self.missing(place))?;
                // Which reader or card this is: paths and sizes alone could
                // otherwise be shared by two different devices.
                let scope = format!(
                    "{}\u{1}{}\u{1}{}",
                    place.label(),
                    match destination {
                        inventory::Destination::Card(card) => card.display.clone(),
                        inventory::Destination::Reader(reader) =>
                            reader.address().host().to_owned(),
                    },
                    destination.base()
                );
                for file in destination.files()?.into_iter().filter(|f| {
                    // The reader's library lists only EPUB, so that is what is
                    // on it as far as anyone reading is concerned. A PDF left
                    // there is a file, not a book.
                    !f.directory && Format::of(&f.path).is_some_and(Format::shown_on_reader)
                }) {
                    // The reader's listing offers a size and nothing else; a
                    // replacement of exactly the same size is not detected.
                    let key = Some(cache::source_key(
                        &scope,
                        &file.path,
                        &file.size.to_string(),
                        settings.optimize,
                        settings.organized,
                    ));
                    match stored(index, key.as_ref(), place, &file.path) {
                        Some(book) => listing.known.push(book),
                        None => listing.todo.push(Item::Reader { file, key }),
                    }
                }
            }
        }
        Ok(listing)
    }
    /// Read one book a listing found and learn what it is. Never fails: a
    /// book that cannot be read is listed as unreadable, with the reason.
    pub fn read(&self, place: Place, item: Item, index: &cache::Index) -> Book {
        let settings = &self.settings;
        match item {
            Item::Local {
                root,
                path,
                source,
                key,
                ..
            } => {
                let name = path.parts().last().cloned().unwrap_or_default();
                let format = Format::of(&name).unwrap_or_default();
                files::book(&*self.folder(root).dir, &path)
                    .and_then(|data| {
                        candidate(
                            data,
                            place,
                            format,
                            source.clone(),
                            settings,
                            index,
                            key,
                            false,
                        )
                    })
                    .unwrap_or_else(|e| unreadable(place, format, source, name, format!("{e:#}")))
            }
            Item::Kobo { book, key } => {
                let store = matches!(book.source, kobo::Source::KoboStore);
                let format = book.format;
                let result = self
                    .kobo
                    .as_ref()
                    .ok_or_else(|| self.missing(place))
                    .and_then(|device| device.import(&book.id, settings.serial.as_deref()))
                    .and_then(|(_, data)| {
                        candidate(
                            data,
                            place,
                            format,
                            book.id.clone(),
                            settings,
                            index,
                            key,
                            store,
                        )
                    });
                result.unwrap_or_else(|e| {
                    unreadable(place, format, book.id, book.title, format!("{e:#}"))
                })
            }
            Item::Reader { file, key } => {
                let format = Format::of(&file.path).unwrap_or_default();
                let result = self
                    .reader
                    .as_ref()
                    .ok_or_else(|| self.missing(place))
                    .and_then(|destination| destination.read(&file))
                    .and_then(|data| {
                        candidate(
                            data,
                            place,
                            format,
                            file.path.clone(),
                            settings,
                            index,
                            key,
                            false,
                        )
                    });
                result.unwrap_or_else(|e| {
                    unreadable(
                        place,
                        format,
                        file.path.clone(),
                        file.path
                            .rsplit('/')
                            .next()
                            .unwrap_or("Unreadable book")
                            .to_owned(),
                        format!("{e:#}"),
                    )
                })
            }
        }
    }
    /// Every place, one after the other: for the command line, which has no
    /// screen to keep alive. The terminal interface runs the same steps as
    /// tasks.
    pub fn scan(&self) -> Snapshot {
        let index = cache::Index::open(self.cache.clone(), self.clock.clone());
        let mut scan = Scan::new();
        for place in [Place::Local, Place::Kobo, Place::CrossPoint] {
            match self.list(place, &index) {
                Ok(listing) => {
                    let todo = scan.listed(place, listing);
                    for item in todo {
                        let book = self.read(place, item, &index);
                        scan.book(place, book);
                    }
                    scan.finished(place, Ok(()));
                }
                Err(e) => scan.finished(place, Err(format!("{e:#}"))),
            }
        }
        index.save();
        scan.snapshot()
    }
}
/// A catalog being put together as locations report. Independent locations
/// complete separately; an unavailable device never erases books found
/// elsewhere.
#[derive(Clone, Debug)]
pub struct Scan {
    snapshot: Snapshot,
    /// Books and unreadable entries per place.
    counts: [(Place, usize, usize); 3],
}
impl Default for Scan {
    fn default() -> Self {
        Self::new()
    }
}
impl Scan {
    pub fn new() -> Self {
        Self {
            snapshot: Snapshot {
                status: vec![
                    (Place::Local, "Checking".into()),
                    (Place::Kobo, "Checking".into()),
                    (Place::CrossPoint, "Checking".into()),
                ],
                ..Snapshot::default()
            },
            counts: [
                (Place::Local, 0, 0),
                (Place::Kobo, 0, 0),
                (Place::CrossPoint, 0, 0),
            ],
        }
    }
    fn count(&mut self, place: Place) -> &mut (Place, usize, usize) {
        self.counts
            .iter_mut()
            .find(|(p, _, _)| *p == place)
            .expect("every place is counted")
    }
    fn status(&mut self, place: Place, text: String) {
        if let Some(status) = self.snapshot.status.iter_mut().find(|(p, _)| *p == place) {
            status.1 = text;
        }
    }
    /// Take what a listing knew already; returns what is left to read.
    pub fn listed(&mut self, place: Place, listing: Listing) -> Vec<Item> {
        self.count(place).2 += listing.unreadable;
        self.snapshot.acsm.extend(listing.acsm);
        for book in listing.known {
            self.book(place, book);
        }
        for book in listing.previews {
            self.add(place, book, false);
        }
        listing.todo
    }
    pub fn book(&mut self, place: Place, book: Book) {
        // An entry whose copies never hashed is a warning, not a usable book.
        let unreadable = !book.copies.is_empty() && book.copies.iter().all(|c| c.sha.is_empty());
        self.add(place, book, unreadable);
    }
    fn add(&mut self, place: Place, book: Book, unreadable: bool) {
        let count = self.count(place);
        count.1 += 1;
        count.2 += usize::from(unreadable);
        let books = count.1;
        merge(&mut self.snapshot.books, book);
        self.status(place, format!("Checking ({})", books_count(books)));
    }
    pub fn finished(&mut self, place: Place, result: Result<(), String>) {
        let (_, count, warnings) = *self.count(place);
        self.status(
            place,
            result.map_or_else(
                |e| {
                    if e.ends_with(files::NOT_CONNECTED) {
                        "Not connected".to_owned()
                    } else {
                        format!("Unavailable: {e}")
                    }
                },
                |()| format!("Ready ({}, {warnings} unreadable)", books_count(count)),
            ),
        );
    }
    /// The catalog so far, sorted.
    pub fn snapshot(&self) -> Snapshot {
        let mut snapshot = self.snapshot.clone();
        sort(&mut snapshot.books);
        snapshot
    }
    pub fn len(&self) -> usize {
        self.snapshot.books.len()
    }
    pub fn is_empty(&self) -> bool {
        self.snapshot.books.is_empty()
    }
}
fn books_count(count: usize) -> String {
    books(count)
}
/// Whether two spellings name the same copy. Devices must match exactly; a
/// local copy is named by its folder and its path inside it, so the same.
fn same_file(recorded: &str, wanted: &str) -> bool {
    recorded == wanted
}
/// Remove one copy of a book, and only when every rule holds: never the last
/// copy anywhere, never a book the Kobo database owns, never over Wi-Fi, and
/// never a file whose contents no longer match what discovery recorded. The
/// file is deleted, not moved to a trash folder: the point is the space.
///
/// Only a mounted reader card is ever given to this program with the right to
/// delete; local folders and the Kobo only gain files.
pub fn remove(
    library: &Library,
    book: &Book,
    place: Place,
    path: &str,
    progress: &dyn Fn(&str),
) -> Result<String> {
    ensure!(
        book.copies.len() > 1,
        "This is the only copy of {}; copy it somewhere else before removing this one",
        book.title
    );
    let copy = book
        .copies
        .iter()
        .find(|c| c.place == place && same_file(&c.path, path))
        .context("That copy is no longer in the library; refresh and try again")?;
    ensure!(
        !copy.locked,
        "The Kobo database lists this book, so its file is not ours to delete; remove it on the Kobo itself"
    );
    ensure!(
        !copy.sha.is_empty(),
        "This copy could not be read during discovery, so it cannot be identified; resolve that first"
    );
    match place {
        Place::Local => anyhow::bail!(
            "Crossload only adds books to {}; delete the file there yourself",
            library.books.display
        ),
        Place::Kobo => {
            anyhow::bail!("Crossload only adds books to the Kobo; delete it on the Kobo itself")
        }
        Place::CrossPoint => {
            progress(&format!("Checking the {} copy…", place.label()));
            let changed =
                "Contents changed since discovery; nothing was deleted. Refresh and look again";
            let destination = library
                .reader
                .as_ref()
                .ok_or_else(|| library.missing(place))?;
            let file = inventory::FileEntry {
                path: copy.path.clone(),
                size: copy.size,
                directory: false,
            };
            ensure!(
                matches!(destination, inventory::Destination::Card(_)),
                "Deleting over Wi-Fi is not supported by the reader's protocol; mount its card and use --copy-to, or delete on the device"
            );
            ensure!(
                inventory::identity(&destination.read(&file)?).sha256 == copy.sha,
                "{changed}"
            );
            progress("Deleting…");
            destination.remove(&file)?;
            Ok(format!(
                "Deleted {} from the reader's card ({})",
                copy.path,
                size(copy.size)
            ))
        }
    }
}
/// How much room a place has left, in bytes.
///
/// A reader reached over Wi-Fi cannot say. CrossPoint's status reports its free
/// heap, which is memory rather than storage, and it has no endpoint for the
/// card; admitting that is better than reporting a number that means something
/// else. A mounted card, a Kobo and a local folder all answer honestly.
pub fn room(library: &Library, place: Place) -> Option<u64> {
    let root = RelPath::root();
    match place {
        Place::Local => library.import.dir.available_space(&root).ok(),
        Place::Kobo => library.kobo.as_ref()?.dir.available_space(&root).ok(),
        Place::CrossPoint => library.card()?.dir.available_space(&root).ok(),
    }
}

/// Human-readable bytes, matching what the interface shows.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
pub fn transfer(
    library: &Library,
    book: &Book,
    target: Place,
    progress: &dyn Fn(&str),
) -> Result<String> {
    let outcome = transferred(library, book, target, progress);
    // Every copy comes through here, so this is where it is remembered. The
    // record is a courtesy: a copy that happened is never reported as failed
    // because a line about it could not be written.
    history::record(
        library.history.as_ref(),
        &history::Entry {
            at: history::now(&*library.clock),
            title: book.title.clone(),
            author: book.author.clone(),
            from: book.preferred().map_or(target, |copy| copy.place),
            to: target,
            bytes: book.preferred().map_or(0, |copy| copy.size),
            ok: outcome.is_ok(),
            detail: match &outcome {
                Ok(message) => message.clone(),
                Err(e) => format!("{e:#}"),
            },
        },
    );
    outcome
}
fn transferred(
    library: &Library,
    book: &Book,
    target: Place,
    progress: &dyn Fn(&str),
) -> Result<String> {
    let settings = &library.settings;
    let source = book.preferred().context("No available copy")?;
    ensure!(!source.sha.is_empty(), "Source could not be verified during discovery; refresh or resolve its read error before copying");
    ensure!(
        source.place != target,
        "Preferred copy is already at this location"
    );
    // The reader lists only EPUB. A PDF can earn its way there by being
    // converted, and the conversion is kept beside the original rather than
    // standing in for it.
    let converting = target == Place::CrossPoint && !source.format.shown_on_reader();
    if converting {
        ensure!(
            source.format == Format::Pdf,
            "The reader's library only lists EPUB, and a {} cannot be converted into one",
            source.format.label()
        );
        ensure!(
            !matches!(source.verdict, Some(crate::pdf::Verdict::Impossible(_))),
            "This PDF cannot be converted: {}",
            source
                .verdict
                .as_ref()
                .map(|verdict| verdict.because())
                .unwrap_or_default()
        );
    }
    progress(&format!("Reading {} copy…", source.place.label()));
    let (name, data) = match source.place {
        Place::Local => {
            let (root, path) = library.locate(&source.path)?;
            let name = path.parts().last().cloned().unwrap_or_default();
            (name, files::book(&*library.folder(root).dir, &path)?)
        }
        Place::Kobo => library
            .kobo
            .as_ref()
            .ok_or_else(|| library.missing(Place::Kobo))?
            .import(&source.path, settings.serial.as_deref())?,
        Place::CrossPoint => {
            let data = library
                .reader
                .as_ref()
                .ok_or_else(|| library.missing(Place::CrossPoint))?
                .read(&inventory::FileEntry {
                    path: source.path.clone(),
                    size: source.size,
                    directory: false,
                })?;
            (format!("book.{}", source.format.extension()), data)
        }
    };
    ensure!(
        inventory::identity(&data).sha256 == source.sha,
        "Source changed since discovery; refresh before copying"
    );
    // Converting publishes the EPUB locally first: it is a book in its own
    // right, and one nobody should have to take on trust unread.
    let (name, data, converted) = if converting {
        progress("Converting to EPUB…");
        let (epub, _) = crate::pdf::convert(&data)?;
        // Name the conversion after the book, not after whatever the PDF's
        // file happened to be called.
        let name = format!("{}.epub", prepare::component(&book.title, "Untitled"));
        let published = copy::copy(&epub, &name, &*library.import.dir, &RelPath::root())?.path;
        let shown = library.source(Root::Import, &published);
        (name, epub, Some(shown))
    } else {
        (name, data, None)
    };
    progress("Preparing device copy…");
    let prepared = prepare::prepare(
        data,
        &name,
        settings.optimize.filter(|_| target == Place::CrossPoint),
        true,
    )?;
    progress(&format!("Copying to {} and verifying…", target.label()));
    let output = match target {
        Place::Local => {
            let copied = copy::copy(
                &prepared.data,
                &prepared.name,
                &*library.import.dir,
                &RelPath::root(),
            )?;
            library.source(Root::Import, &copied.path)
        }
        Place::Kobo => {
            let device = library
                .kobo
                .as_ref()
                .ok_or_else(|| library.missing(Place::Kobo))?;
            // Require the mounted Kobo database; never write to a folder that
            // only looks like a Kobo.
            device.books()?;
            let directory =
                prepare::author_directory(&*device.dir, &RelPath::root(), &prepared.author)?;
            // A kepub is read by the engine the store's own books get, which is
            // what makes progress and reading statistics work.
            let (name, data) = if settings.kepub && source.format.rewritten() {
                progress("Preparing a kepub for the Kobo…");
                let stem = prepared
                    .name
                    .rsplit_once('.')
                    .map_or(prepared.name.as_str(), |(stem, _)| stem);
                (
                    crate::kepub::name(stem),
                    crate::kepub::kepubify(&prepared.data)?,
                )
            } else {
                (prepared.name.clone(), prepared.data.clone())
            };
            let copied = copy::copy(&data, &name, &*device.dir, &directory)?;
            format!(
                "{}{}",
                device.display.trim_end_matches('/'),
                files::slash(&copied.path)
            )
        }
        Place::CrossPoint => match library
            .reader
            .as_ref()
            .ok_or_else(|| library.missing(Place::CrossPoint))?
        {
            inventory::Destination::Reader(reader) => {
                let reader = reader.clone().prepared_for(settings.optimize);
                let reader = if settings.organized {
                    reader.for_author(&prepared.author)?
                } else {
                    reader
                };
                let sent = reader.send_with_progress(&prepared.name, &prepared.data, progress)?;
                if let Some(note) = &sent.note {
                    progress(note);
                }
                sent.path
            }
            inventory::Destination::Card(card) => {
                let folder = if settings.organized {
                    prepare::author_directory(&*card.dir, &RelPath::root(), &prepared.author)?
                } else {
                    RelPath::root()
                };
                let copied = copy::copy(&prepared.data, &prepared.name, &*card.dir, &folder)?;
                format!(
                    "{}{}",
                    card.display.trim_end_matches('/'),
                    files::slash(&copied.path)
                )
            }
        },
    };
    if let Some(published) = converted {
        return Ok(format!(
            "Converted to {published} and copied to {output} (verified). The PDF is untouched."
        ));
    }
    Ok(format!(
        "Copied to {output} (verified).{}",
        if source.optimized {
            " Source is a device copy; any image optimization in it cannot be undone."
        } else {
            ""
        }
    ))
}
/// Correct what a local EPUB says about itself. Folders only gain files, so
/// the corrected book is added to the import folder and the original is left
/// as it is; the two count as one book, since only the metadata differs.
pub fn correct(library: &Library, source: &str, edit: &crate::metadata::Edit) -> Result<String> {
    let (root, path) = library.locate(source)?;
    let data = files::book(&*library.folder(root).dir, &path)?;
    let changed = crate::metadata::apply(&data, edit)?;
    let name = path.parts().last().cloned().unwrap_or_default();
    let stem = name
        .rsplit_once('.')
        .map_or(name.as_str(), |(stem, _)| stem);
    let dir = &*library.import.dir;
    for attempt in 0..100 {
        let candidate = match attempt {
            0 if root == Root::Books => name.clone(),
            0 => continue,
            1 => format!("{stem} (corrected).epub"),
            n => format!("{stem} (corrected {n}).epub"),
        };
        let target = RelPath::new([candidate.as_str()])?;
        match dir.create_new(&target, &changed) {
            Ok(()) => {
                return Ok(format!(
                    "Saved the corrected book as {}; {source} is unchanged",
                    library.source(Root::Import, &target)
                ))
            }
            Err(e) if e.kind == ambit_caps_dir::DirErrorKind::Exists => continue,
            Err(e) => return Err(anyhow::Error::new(e).context("Cannot save the corrected book")),
        }
    }
    anyhow::bail!("Too many corrected copies of {name} in the import folder")
}
