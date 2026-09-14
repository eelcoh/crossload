//! Best-effort identity cache for discovery.
//!
//! A hit only supplies identity for a file the current scan has already
//! enumerated, so the catalog stays a fresh inventory and never becomes an
//! offline history of disconnected devices. Every failure — missing, corrupt,
//! unreadable or unwritable — degrades to recomputing, never to an error.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
/// Bumped whenever a stored field changes meaning; older files are discarded.
const VERSION: u32 = 1;
const MAX_ENTRIES: usize = 20_000;
const MAX_AGE: u64 = 90 * 24 * 60 * 60;
/// What discovery would otherwise reread and rehash the whole book to learn.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Source {
    pub title: String,
    pub author: String,
    pub sha: String,
    pub resources: Option<String>,
    pub optimized: bool,
    pub variants: Vec<(String, Option<String>)>,
}
/// The identity of the optimized copy that these bytes would produce.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Variant {
    pub sha: String,
    pub resources: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Aged<T> {
    #[serde(flatten)]
    value: T,
    seen: u64,
}
#[derive(Default, Serialize, Deserialize)]
struct Stored {
    version: u32,
    #[serde(default)]
    sources: BTreeMap<String, Aged<Source>>,
    #[serde(default)]
    variants: BTreeMap<String, Aged<Variant>>,
}
struct State {
    stored: Stored,
    dirty: bool,
}
pub struct Index {
    path: Option<PathBuf>,
    state: Mutex<State>,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}
pub fn default_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let base = if cfg!(target_os = "macos") {
        PathBuf::from(home).join("Library/Caches")
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| PathBuf::from(home).join(".cache"))
    };
    Some(base.join("crossload").join("index.json"))
}
/// A file's identity key. Any change to its size or modification time is a
/// miss, as is a change to the options that shaped the stored variants.
pub fn source_key(
    place: &str,
    path: &Path,
    optimize: bool,
    organized: bool,
) -> Option<(String, u64)> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some((
        format!(
            "{place}\u{1}{}\u{1}{}\u{1}{}.{:09}\u{1}{optimize}{organized}",
            path.display(),
            metadata.len(),
            modified.as_secs(),
            modified.subsec_nanos(),
        ),
        metadata.len(),
    ))
}
pub fn variant_key(sha: &str, optimize: bool, organized: bool) -> String {
    format!("{sha}\u{1}{optimize}{organized}")
}
impl Index {
    /// `path` of `None` uses the default cache file; a path that cannot be read
    /// or parsed starts empty rather than failing.
    pub fn open(path: Option<PathBuf>) -> Self {
        let path = path.or_else(default_path);
        let stored = path
            .as_ref()
            .and_then(|path| fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<Stored>(&bytes).ok())
            .filter(|stored| stored.version == VERSION)
            .unwrap_or_default();
        Self {
            path,
            state: Mutex::new(State {
                stored,
                dirty: false,
            }),
        }
    }
    pub fn source(&self, key: &str) -> Option<Source> {
        let mut state = self.state.lock().ok()?;
        let seen = now();
        let entry = state.stored.sources.get_mut(key)?;
        entry.seen = seen;
        Some(entry.value.clone())
    }
    pub fn put_source(&self, key: String, value: Source) {
        if let Ok(mut state) = self.state.lock() {
            let seen = now();
            state.stored.sources.insert(key, Aged { value, seen });
            state.dirty = true;
        }
    }
    pub fn variant(&self, key: &str) -> Option<Variant> {
        let mut state = self.state.lock().ok()?;
        let seen = now();
        let entry = state.stored.variants.get_mut(key)?;
        entry.seen = seen;
        Some(entry.value.clone())
    }
    pub fn put_variant(&self, key: String, value: Variant) {
        if let Ok(mut state) = self.state.lock() {
            let seen = now();
            state.stored.variants.insert(key, Aged { value, seen });
            state.dirty = true;
        }
    }
    /// Write the index back, replacing it atomically. Nothing here can fail a
    /// scan: a cache that cannot be stored simply costs the next scan its work.
    pub fn save(&self) {
        let (Some(path), Ok(mut state)) = (self.path.as_ref(), self.state.lock()) else {
            return;
        };
        if !state.dirty {
            return;
        }
        let cutoff = now().saturating_sub(MAX_AGE);
        state.stored.sources.retain(|_, entry| entry.seen >= cutoff);
        state
            .stored
            .variants
            .retain(|_, entry| entry.seen >= cutoff);
        newest(&mut state.stored.sources);
        newest(&mut state.stored.variants);
        state.stored.version = VERSION;
        let Ok(bytes) = serde_json::to_vec(&state.stored) else {
            return;
        };
        let Some(directory) = path.parent() else {
            return;
        };
        if fs::create_dir_all(directory).is_err() {
            return;
        }
        let Ok(mut file) = tempfile::NamedTempFile::new_in(directory) else {
            return;
        };
        if file.write_all(&bytes).is_ok() && file.as_file().sync_all().is_ok() {
            let _ = file.persist(path);
        }
        state.dirty = false;
    }
}
/// Keep the most recently used entries when the index outgrows its bound.
fn newest<T>(entries: &mut BTreeMap<String, Aged<T>>) {
    if entries.len() <= MAX_ENTRIES {
        return;
    }
    let mut seen: Vec<u64> = entries.values().map(|entry| entry.seen).collect();
    seen.sort_unstable();
    let cutoff = seen[seen.len() - MAX_ENTRIES];
    let mut kept = 0;
    entries.retain(|_, entry| {
        let keep = entry.seen >= cutoff && kept < MAX_ENTRIES;
        kept += usize::from(keep);
        keep
    });
}
