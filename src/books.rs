//! A content-based library across local storage, Kobo and CrossPoint.
use crate::{
    cache, copy, crosspoint::Reader, epub, format, format::Format, inventory, kobo, prepare,
};
use anyhow::{ensure, Context, Result};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
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
#[derive(Clone)]
pub struct Options {
    pub show_previews: bool,
    pub local: PathBuf,
    pub output: PathBuf,
    pub kobo: Option<PathBuf>,
    pub reader: Option<String>,
    pub card: Option<PathBuf>,
    pub folder: String,
    pub serial: Option<String>,
    pub optimize: bool,
    pub organized: bool,
    /// Identity cache file; None uses the default cache location.
    pub cache: Option<PathBuf>,
}
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub books: Vec<Book>,
    pub status: Vec<(Place, String)>,
    pub acsm: Vec<PathBuf>,
}
impl Snapshot {
    pub fn ready(&self, place: Place) -> bool {
        self.status
            .iter()
            .any(|(p, s)| *p == place && s.starts_with("Ready"))
    }
}
fn bytes(path: &Path) -> Result<Vec<u8>> {
    let file = fs::File::open(path)?;
    ensure!(file.metadata()?.is_file(), "Not a regular book file");
    let mut data = Vec::new();
    file.take(epub::MAX_BOOK_BYTES + 1).read_to_end(&mut data)?;
    ensure!(
        data.len() as u64 <= epub::MAX_BOOK_BYTES,
        "Book exceeds 128 MiB"
    );
    Ok(data)
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
    path: &Path,
    place: Place,
    format: Format,
    source: String,
    options: &Options,
    index: &cache::Index,
    key: Option<String>,
    locked: bool,
) -> Result<Book> {
    let data = bytes(path)?;
    format::validate(format, &data)?;
    let (title, author, series, series_index) = if format.rewritten() {
        let metadata = epub::metadata(path)?.context("Missing EPUB metadata")?;
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
        let key = cache::variant_key(&id.sha256, options.optimize, options.organized);
        let variant = match index.variant(&key) {
            Some(variant) => variant,
            None => {
                let prepared = prepare::prepare(path, options.optimize, options.organized)?;
                let identity = inventory::identity(&bytes(&prepared.path)?);
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
struct Budget {
    free: std::sync::Mutex<u64>,
    released: std::sync::Condvar,
}
struct Claim<'a> {
    budget: &'a Budget,
    amount: u64,
}
impl Budget {
    /// Enough for several ordinary books at once, and a ceiling a single very
    /// large one cannot exceed.
    const TOTAL: u64 = 384 * 1024 * 1024;
    fn new() -> Self {
        Self {
            free: std::sync::Mutex::new(Self::TOTAL),
            released: std::sync::Condvar::new(),
        }
    }
    fn claim(&self, size: u64) -> Claim<'_> {
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
/// What a location reports while it works, rather than only when it finishes.
enum Update {
    Book(Place, Box<Book>, bool),
    Acsm(PathBuf),
    Unreadable(Place),
    Finished(Place, Result<(), String>),
}
type Reports = std::sync::mpsc::Sender<Update>;
fn book_update(place: Place, book: Book) -> Update {
    // An entry whose copies never hashed is a warning, not a usable book.
    let unreadable = !book.copies.is_empty() && book.copies.iter().all(|c| c.sha.is_empty());
    Update::Book(place, Box::new(book), unreadable)
}
fn discover(options: &Options, place: Place, index: &cache::Index, tx: &Reports) -> Result<()> {
    match place {
        Place::Local => {
            let mut roots = vec![options.local.clone()];
            if options.output != options.local && options.output.exists() {
                roots.push(options.output.clone());
            }
            ensure!(
                roots.iter().any(|p| p.is_dir()),
                "Local folders unavailable"
            );
            // Walking a tree is cheap; reading and hashing books is not. The
            // walk finishes first, then the books are read in parallel.
            let mut seen = std::collections::HashSet::new();
            let mut visited = 0;
            let mut books = Vec::new();
            for root in roots {
                let mut pending = vec![(root, 0)];
                while let Some((dir, depth)) = pending.pop() {
                    ensure!(depth <= 32, "Local tree exceeds 32 levels");
                    let items = match fs::read_dir(&dir) {
                        Ok(items) => items,
                        Err(_) => {
                            let _ = tx.send(Update::Unreadable(place));
                            continue;
                        }
                    };
                    for item in items {
                        let item = item?;
                        visited += 1;
                        ensure!(
                            visited <= 20000,
                            "Local tree exceeds 20000 entries; choose a narrower --browse folder"
                        );
                        let kind = item.file_type()?;
                        if kind.is_symlink() || item.file_name().to_string_lossy().starts_with('.')
                        {
                            continue;
                        }
                        if kind.is_dir() {
                            pending.push((item.path(), depth + 1));
                            continue;
                        }
                        if !kind.is_file() {
                            continue;
                        }
                        let path = item.path().canonicalize()?;
                        if !seen.insert(path.clone()) {
                            continue;
                        }
                        let ext = path
                            .extension()
                            .and_then(|s| s.to_str())
                            .unwrap_or("")
                            .to_ascii_lowercase();
                        if matches!(ext.as_str(), "acsm" | "ascm") {
                            // A fulfilled request is moved into archive/ beside
                            // itself; it is spent, not pending.
                            let archived = path.parent().and_then(|p| p.file_name())
                                == Some(std::ffi::OsStr::new("archive"));
                            if !archived {
                                let _ = tx.send(Update::Acsm(path));
                            }
                            continue;
                        }
                        if Format::ALL.iter().any(|f| f.extension() == ext) {
                            books.push(path);
                        }
                    }
                }
            }
            let budget = Budget::new();
            let budget = &budget;
            let next = std::sync::atomic::AtomicUsize::new(0);
            let workers = std::thread::available_parallelism()
                .map(|value| value.get())
                .unwrap_or(1)
                .min(8)
                .min(books.len().max(1));
            let books = &books;
            let next = &next;
            std::thread::scope(|scope| {
                for worker in 0..workers {
                    let tx = tx.clone();
                    let read = move || {
                        while let Some(path) =
                            books.get(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
                        {
                            let source = path.to_string_lossy().into_owned();
                            let key = cache::file_fingerprint(path).map(|fingerprint| {
                                cache::source_key(
                                    place.label(),
                                    &source,
                                    &fingerprint,
                                    options.optimize,
                                    options.organized,
                                )
                            });
                            let format = Format::of_path(path).unwrap_or_default();
                            let book = match stored(index, key.as_ref(), place, &source) {
                                // A cached identity costs nothing to hold.
                                Some(book) => Ok(book),
                                None => {
                                    let _claim = budget
                                        .claim(fs::metadata(path).map(|m| m.len()).unwrap_or(0));
                                    candidate(
                                        path,
                                        place,
                                        format,
                                        source.clone(),
                                        options,
                                        index,
                                        key,
                                        false,
                                    )
                                }
                            }
                            .unwrap_or_else(|e| {
                                unreadable(
                                    place,
                                    format,
                                    source,
                                    path.file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .into_owned(),
                                    format!("{e:#}"),
                                )
                            });
                            let _ = tx.send(book_update(place, book));
                        }
                    };
                    std::thread::Builder::new()
                        .name(format!("read-local-{worker}"))
                        .spawn_scoped(scope, read)
                        .expect("cannot start a reading thread");
                }
            });
        }
        Place::Kobo => {
            let root = options.kobo.as_ref().context("Not configured")?;
            let library = kobo::Library::open(root)?;
            for book in library
                .books()?
                .into_iter()
                .filter(|b| options.show_previews || !b.preview)
            {
                if book.preview {
                    // A preview is a known state, not a failed read.
                    let _ = tx.send(Update::Book(
                        place,
                        Box::new(unreadable(
                            place,
                            Format::Epub,
                            book.id,
                            format!("[Preview] {}", book.title),
                            "Download the full book on Kobo first".into(),
                        )),
                        false,
                    ));
                    continue;
                }
                // The decrypted copy follows from the device file, the serial
                // and this library, so an unchanged file needs no import.
                let scope = format!(
                    "{}\u{1}{}\u{1}{}",
                    place.label(),
                    root.display(),
                    options.serial.as_deref().unwrap_or_default()
                );
                let key = library
                    .source_path(&book)
                    .ok()
                    .as_deref()
                    .and_then(cache::file_fingerprint)
                    .map(|fingerprint| {
                        cache::source_key(
                            &scope,
                            &book.id,
                            &fingerprint,
                            options.optimize,
                            options.organized,
                        )
                    });
                if let Some(found_book) = stored(index, key.as_ref(), place, &book.id) {
                    let _ = tx.send(book_update(place, found_book));
                    continue;
                }
                let store = matches!(book.source, kobo::Source::KoboStore);
                let format = book.format;
                let temp = tempfile::tempdir()?;
                let result = library
                    .import(&book.id, temp.path(), options.serial.as_deref())
                    .and_then(|p| {
                        candidate(
                            &p,
                            place,
                            format,
                            book.id.clone(),
                            options,
                            index,
                            key,
                            store,
                        )
                    });
                let _ = tx.send(book_update(
                    place,
                    result.unwrap_or_else(|e| {
                        unreadable(place, format, book.id, book.title, format!("{e:#}"))
                    }),
                ));
            }
        }
        Place::CrossPoint => {
            let destination = destination(options)?;
            // Which reader or card this is: paths and sizes alone could
            // otherwise be shared by two different devices.
            let scope = format!(
                "{}\u{1}{}\u{1}{}",
                place.label(),
                options
                    .card
                    .as_ref()
                    .map(|card| card.display().to_string())
                    .or_else(|| options.reader.clone())
                    .unwrap_or_default(),
                options.folder
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
                    options.optimize,
                    options.organized,
                ));
                if let Some(book) = stored(index, key.as_ref(), place, &file.path) {
                    let _ = tx.send(book_update(place, book));
                    continue;
                }
                let format = Format::of(&file.path).unwrap_or_default();
                let result = (|| -> Result<Book> {
                    let data = destination.read(&file)?;
                    let temp = tempfile::tempdir()?;
                    let path = temp.path().join(format!("book.{}", format.extension()));
                    fs::write(&path, data)?;
                    candidate(
                        &path,
                        place,
                        format,
                        file.path.clone(),
                        options,
                        index,
                        key,
                        false,
                    )
                })();
                let _ = tx.send(book_update(
                    place,
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
                    }),
                ));
            }
        }
    }
    Ok(())
}
/// Independent locations complete separately; an unavailable device never erases
/// books found elsewhere. No discovery step writes to a library or device.
pub fn scan(options: &Options, mut progress: impl FnMut(Snapshot)) -> Snapshot {
    /// How often a partial catalog is published while locations are working.
    const INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);
    let mut snapshot = Snapshot {
        status: vec![
            (Place::Local, "Checking".into()),
            (Place::Kobo, "Checking".into()),
            (Place::CrossPoint, "Checking".into()),
        ],
        ..Snapshot::default()
    };
    let mut counts = [
        (Place::Local, 0, 0),
        (Place::Kobo, 0, 0),
        (Place::CrossPoint, 0, 0),
    ];
    let index = cache::Index::open(options.cache.clone());
    std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::channel();
        for place in [Place::Local, Place::Kobo, Place::CrossPoint] {
            let tx = tx.clone();
            let index = &index;
            let worker = move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    discover(options, place, index, &tx)
                }))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("Location worker stopped")));
                let _ = tx.send(Update::Finished(
                    place,
                    result.map_err(|e| format!("{e:#}")),
                ));
            };
            // Naming costs nothing and makes a busy scan identifiable from
            // outside; a failure to spawn panics exactly as scope.spawn does.
            std::thread::Builder::new()
                .name(format!("scan-{}", place.label().to_lowercase()))
                .spawn_scoped(scope, worker)
                .expect("cannot start a discovery thread");
        }
        drop(tx);
        let mut last = std::time::Instant::now();
        let mut shown = 0;
        for update in rx {
            let mut finished = false;
            let mut status = None;
            match update {
                Update::Book(place, book, unreadable) => {
                    let count = counts.iter_mut().find(|(p, _, _)| *p == place).unwrap();
                    count.1 += 1;
                    count.2 += usize::from(unreadable);
                    merge(&mut snapshot.books, *book);
                    status = Some((place, format!("Checking ({})", books(count.1))));
                }
                Update::Acsm(path) => snapshot.acsm.push(path),
                Update::Unreadable(place) => {
                    counts.iter_mut().find(|(p, _, _)| *p == place).unwrap().2 += 1;
                }
                Update::Finished(place, result) => {
                    finished = true;
                    let (_, count, warnings) =
                        *counts.iter().find(|(p, _, _)| *p == place).unwrap();
                    status = Some((
                        place,
                        result.map_or_else(
                            |e| format!("Unavailable: {e}"),
                            |()| format!("Ready ({}, {warnings} unreadable)", books(count)),
                        ),
                    ));
                }
            }
            if let Some((place, text)) = status {
                snapshot
                    .status
                    .iter_mut()
                    .find(|(p, _)| *p == place)
                    .unwrap()
                    .1 = text;
            }
            // A location's own result is always published. Between those, the
            // first books appear at once and the rest at a readable rate.
            let first = shown == 0 && !snapshot.books.is_empty();
            if finished || first || last.elapsed() >= INTERVAL {
                sort(&mut snapshot.books);
                shown = snapshot.books.len();
                last = std::time::Instant::now();
                progress(snapshot.clone());
            }
        }
    });
    index.save();
    sort(&mut snapshot.books);
    snapshot
}
fn destination(options: &Options) -> Result<inventory::Destination> {
    if let Some(card) = &options.card {
        crate::sync::card(card)
    } else {
        Ok(inventory::Destination::Reader(Reader::new(
            options.reader.as_deref().context("Not configured")?,
            &options.folder,
        )?))
    }
}
/// Whether two spellings name the same copy. Local paths are compared as files,
/// since the same file can be reached by more than one path: a relative one, or
/// a directory that is a symlink, as `/var` is on macOS. Devices are not
/// filesystem paths at all and must match exactly.
fn same_file(place: Place, recorded: &str, wanted: &str) -> bool {
    if recorded == wanted {
        return true;
    }
    place == Place::Local
        && matches!(
            (fs::canonicalize(recorded), fs::canonicalize(wanted)),
            (Ok(a), Ok(b)) if a == b
        )
}
/// Remove one copy of a book, and only when every rule holds: never the last
/// copy anywhere, never a book the Kobo database owns, never over Wi-Fi, and
/// never a file whose contents no longer match what discovery recorded. The
/// file is deleted, not moved to a trash folder: the point is the space.
pub fn remove(
    options: &Options,
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
        .find(|c| c.place == place && same_file(place, &c.path, path))
        .context("That copy is no longer in the library; refresh and try again")?;
    ensure!(
        !copy.locked,
        "The Kobo database lists this book, so its file is not ours to delete; remove it on the Kobo itself"
    );
    ensure!(
        !copy.sha.is_empty(),
        "This copy could not be read during discovery, so it cannot be identified; resolve that first"
    );
    progress(&format!("Checking the {} copy…", place.label()));
    let changed = "Contents changed since discovery; nothing was deleted. Refresh and look again";
    match place {
        Place::Local => {
            let file = PathBuf::from(&copy.path);
            ensure!(
                inventory::identity(&bytes(&file)?).sha256 == copy.sha,
                "{changed}"
            );
            ensure!(
                fs::symlink_metadata(&file)?.is_file(),
                "Refusing to remove anything but a regular file"
            );
            progress("Deleting…");
            fs::remove_file(&file)?;
            Ok(format!("Deleted {} ({})", file.display(), size(copy.size)))
        }
        Place::Kobo => {
            let root = options.kobo.as_ref().context("No Kobo configured")?;
            let library = kobo::Library::open(root)?;
            let entry = library
                .books()?
                .into_iter()
                .find(|b| b.id == copy.path)
                .context("That book is no longer on the Kobo; refresh and try again")?;
            ensure!(
                matches!(entry.source, kobo::Source::Sideloaded),
                "The Kobo database lists this book; remove it on the Kobo itself"
            );
            let file = library.source_path(&entry)?;
            ensure!(
                inventory::identity(&bytes(&file)?).sha256 == copy.sha,
                "{changed}"
            );
            progress("Deleting…");
            fs::remove_file(&file)?;
            Ok(format!(
                "Deleted {} from the Kobo ({}). Eject it so the library updates",
                file.display(),
                size(copy.size)
            ))
        }
        Place::CrossPoint => {
            let destination = destination(options)?;
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
pub fn room(path: Option<&Path>) -> Option<u64> {
    // A folder yet to be made has the room of the filesystem it will sit on.
    let existing = path?.ancestors().find(|parent| parent.exists())?;
    fs2::available_space(existing).ok()
}
/// Where each place writes, for asking it how much room is left.
pub fn destination_path(options: &Options, place: Place) -> Option<&Path> {
    match place {
        Place::Local => Some(&options.output),
        Place::Kobo => options.kobo.as_deref(),
        Place::CrossPoint => options.card.as_deref(),
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
    options: &Options,
    book: &Book,
    target: Place,
    progress: &dyn Fn(&str),
) -> Result<String> {
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
    let staging = tempfile::tempdir()?;
    let path = match source.place {
        Place::Local => PathBuf::from(&source.path),
        Place::Kobo => kobo::Library::open(options.kobo.as_ref().context("No Kobo configured")?)?
            .import(&source.path, staging.path(), options.serial.as_deref())?,
        Place::CrossPoint => {
            let data = destination(options)?.read(&inventory::FileEntry {
                path: source.path.clone(),
                size: source.size,
                directory: false,
            })?;
            let path = staging
                .path()
                .join(format!("book.{}", source.format.extension()));
            fs::write(&path, data)?;
            path
        }
    };
    ensure!(
        inventory::identity(&bytes(&path)?).sha256 == source.sha,
        "Source changed since discovery; refresh before copying"
    );
    // Converting publishes the EPUB locally first: it is a book in its own
    // right, and one nobody should have to take on trust unread.
    let (path, converted) = if converting {
        progress("Converting to EPUB…");
        let (epub, _) = crate::pdf::convert(&bytes(&path)?)?;
        // Name the conversion after the book, not after whatever the PDF's
        // file happened to be called.
        let staged = staging.path().join(format!(
            "{}.epub",
            prepare::component(&book.title, "Untitled")
        ));
        fs::write(&staged, &epub)?;
        fs::create_dir_all(&options.output)?;
        let published = copy::copy(&staged, &options.output)?.path;
        (published.clone(), Some(published))
    } else {
        (path, None)
    };
    let prepared = prepare::prepare(&path, target == Place::CrossPoint && options.optimize, true)?;
    progress(&format!("Copying to {} and verifying…", target.label()));
    let output = match target {
        Place::Local => {
            if let Some(kobo) = options.kobo.as_ref().filter(|p| p.exists()) {
                crate::kobo::output_directory(&options.output, &kobo.canonicalize()?)?;
            } else {
                fs::create_dir_all(&options.output)?;
            }
            copy::copy(&prepared.path, &options.output)?
                .path
                .display()
                .to_string()
        }
        Place::Kobo => {
            let root = options.kobo.as_ref().context("No Kobo configured")?;
            // Require the mounted Kobo database; never recreate a disconnected mount.
            kobo::Library::open(root)?;
            let directory = prepare::author_directory(root, &prepared.author)?;
            copy::copy(&prepared.path, &directory)?
                .path
                .display()
                .to_string()
        }
        Place::CrossPoint => match destination(options)? {
            inventory::Destination::Reader(reader) => {
                let reader = if options.organized {
                    reader.for_author(&prepared.author)?
                } else {
                    reader
                };
                reader.send(&prepared.path)?.path
            }
            inventory::Destination::Card(root) => {
                let folder = if options.organized {
                    prepare::author_directory(&root, &prepared.author)?
                } else {
                    root
                };
                copy::copy(&prepared.path, &folder)?
                    .path
                    .display()
                    .to_string()
            }
        },
    };
    if let Some(published) = converted {
        return Ok(format!(
            "Converted to {} and copied to {output} (verified). The PDF is untouched.",
            published.display()
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
