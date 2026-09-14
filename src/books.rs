//! A content-based library across local storage, Kobo and CrossPoint.
use crate::{cache, copy, crosspoint::Reader, epub, inventory, kobo, prepare};
use anyhow::{ensure, Context, Result};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Local,
    Kobo,
    Xteink,
}
impl Place {
    pub fn label(self) -> &'static str {
        match self {
            Self::Local => "Local",
            Self::Kobo => "Kobo",
            Self::Xteink => "Xteink",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Copy {
    pub place: Place,
    pub path: String,
    pub size: u64,
    pub sha: String,
    pub resources: Option<String>,
    pub optimized: bool,
    variants: Vec<(String, Option<String>)>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Book {
    pub title: String,
    pub author: String,
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
                    Place::Xteink => 2,
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
        copies: vec![Copy {
            place,
            path,
            size: entry.size,
            sha: entry.sha,
            resources: entry.resources,
            optimized: entry.optimized,
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
fn candidate(
    path: &Path,
    place: Place,
    source: String,
    options: &Options,
    index: &cache::Index,
    key: Option<String>,
) -> Result<Book> {
    let data = bytes(path)?;
    epub::validate(&data)?;
    let metadata = epub::metadata(path)?.context("Missing EPUB metadata")?;
    let id = inventory::identity(&data);
    let optimized = place == Place::Xteink
        || zip::ZipArchive::new(std::io::Cursor::new(&data))?
            .by_name("META-INF/xteink-device-profile.txt")
            .is_ok();
    let mut variants = vec![(id.sha256.clone(), id.resources.clone())];
    if place != Place::Xteink {
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
    let entry = cache::Source {
        title: metadata.title.unwrap_or_else(|| "Untitled".into()),
        author: metadata.author,
        size: data.len() as u64,
        sha: id.sha256,
        resources: id.resources,
        optimized,
        variants,
    };
    if let Some(key) = key {
        index.put_source(key, entry.clone());
    }
    Ok(book(entry, place, source))
}
fn unreadable(place: Place, path: String, title: String, reason: String) -> Book {
    Book {
        title,
        author: format!("Unreadable: {reason}"),
        copies: vec![Copy {
            place,
            path,
            size: 0,
            sha: String::new(),
            resources: None,
            optimized: place == Place::Xteink,
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
fn books(count: usize) -> String {
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
                            let _ = tx.send(Update::Acsm(path));
                            continue;
                        }
                        if ext == "epub" {
                            books.push(path);
                        }
                    }
                }
            }
            let next = std::sync::atomic::AtomicUsize::new(0);
            let workers = std::thread::available_parallelism()
                .map(|value| value.get())
                .unwrap_or(1)
                .min(8)
                .min(books.len().max(1));
            let books = &books;
            let next = &next;
            std::thread::scope(|scope| {
                for _ in 0..workers {
                    let tx = tx.clone();
                    scope.spawn(move || {
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
                            let book = stored(index, key.as_ref(), place, &source)
                                .map(Ok)
                                .unwrap_or_else(|| {
                                    candidate(path, place, source.clone(), options, index, key)
                                })
                                .unwrap_or_else(|e| {
                                    unreadable(
                                        place,
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
                    });
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
                let temp = tempfile::tempdir()?;
                let result = library
                    .import(&book.id, temp.path(), options.serial.as_deref())
                    .and_then(|p| candidate(&p, place, book.id.clone(), options, index, key));
                let _ = tx.send(book_update(
                    place,
                    result.unwrap_or_else(|e| {
                        unreadable(place, book.id, book.title, format!("{e:#}"))
                    }),
                ));
            }
        }
        Place::Xteink => {
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
            for file in destination
                .files()?
                .into_iter()
                .filter(|f| !f.directory && f.path.to_lowercase().ends_with(".epub"))
            {
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
                let result = (|| -> Result<Book> {
                    let data = destination.read(&file)?;
                    let temp = tempfile::tempdir()?;
                    let path = temp.path().join("book.epub");
                    fs::write(&path, data)?;
                    candidate(&path, place, file.path.clone(), options, index, key)
                })();
                let _ = tx.send(book_update(
                    place,
                    result.unwrap_or_else(|e| {
                        unreadable(
                            place,
                            file.path.clone(),
                            file.path
                                .rsplit('/')
                                .next()
                                .unwrap_or("Unreadable EPUB")
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
            (Place::Xteink, "Checking".into()),
        ],
        ..Snapshot::default()
    };
    let mut counts = [
        (Place::Local, 0, 0),
        (Place::Kobo, 0, 0),
        (Place::Xteink, 0, 0),
    ];
    let index = cache::Index::open(options.cache.clone());
    std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::channel();
        for place in [Place::Local, Place::Kobo, Place::Xteink] {
            let tx = tx.clone();
            let index = &index;
            scope.spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    discover(options, place, index, &tx)
                }))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("Location worker stopped")));
                let _ = tx.send(Update::Finished(
                    place,
                    result.map_err(|e| format!("{e:#}")),
                ));
            });
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
    progress(&format!("Reading {} copy…", source.place.label()));
    let staging = tempfile::tempdir()?;
    let path = match source.place {
        Place::Local => PathBuf::from(&source.path),
        Place::Kobo => kobo::Library::open(options.kobo.as_ref().context("No Kobo configured")?)?
            .import(&source.path, staging.path(), options.serial.as_deref())?,
        Place::Xteink => {
            let data = destination(options)?.read(&inventory::FileEntry {
                path: source.path.clone(),
                size: source.size,
                directory: false,
            })?;
            let path = staging.path().join("book.epub");
            fs::write(&path, data)?;
            path
        }
    };
    ensure!(
        inventory::identity(&bytes(&path)?).sha256 == source.sha,
        "Source changed since discovery; refresh before copying"
    );
    let prepared = prepare::prepare(&path, target == Place::Xteink && options.optimize, true)?;
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
        Place::Xteink => match destination(options)? {
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
    Ok(format!(
        "Copied to {output} (verified).{} Press r to refresh.",
        if source.optimized || source.place == Place::Xteink {
            " Source is a device copy; original quality cannot be restored."
        } else {
            ""
        }
    ))
}
