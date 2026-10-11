//! Best-effort identity cache for discovery.
//!
//! A hit only supplies identity for a file the current scan has already
//! enumerated, so the catalog stays a fresh inventory and never becomes an
//! offline history of disconnected devices. Every failure — missing, corrupt,
//! unreadable or unwritable — degrades to recomputing, never to an error.
use ambit_caps::Clock;
use ambit_caps_dir::{DirModify, Meta, RelPath};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc, sync::Mutex, time::UNIX_EPOCH};
/// Bumped whenever a stored field changes meaning; older files are discarded.
const VERSION: u32 = 8;
const MAX_ENTRIES: usize = 20_000;
const MAX_AGE: u64 = 90 * 24 * 60 * 60;
/// What discovery would otherwise reread and rehash the whole book to learn.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Source {
    pub title: String,
    pub author: String,
    pub format: crate::format::Format,
    /// The series the book belongs to, and where it sits in it.
    #[serde(default)]
    pub series: Option<String>,
    #[serde(default)]
    pub series_index: Option<f32>,
    /// For a PDF, what converting it to EPUB would be worth. Judged once, when
    /// the file is read, so nothing has to open a PDF to draw a dialog.
    #[serde(default)]
    pub verdict: Option<crate::pdf::Verdict>,
    pub size: u64,
    pub sha: String,
    pub resources: Option<String>,
    pub optimized: bool,
    pub locked: bool,
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
/// Where the index is kept: a file in crossload's own cache folder.
#[derive(Clone)]
pub struct Store {
    pub dir: Arc<dyn DirModify>,
    pub file: RelPath,
}
pub struct Index {
    store: Option<Store>,
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
}
/// The name the index has always had in its folder.
pub const FILE: &str = "index.json";
/// An identity key. `scope` separates devices and libraries that could
/// otherwise share an id, and `fingerprint` is whatever cheap evidence that
/// location offers that the source is unchanged. Any difference is a miss, as
/// is a change to the options that shaped the stored variants.
pub fn source_key(
    scope: &str,
    id: &str,
    fingerprint: &str,
    optimize: Option<crate::profile::Profile>,
    organized: bool,
) -> String {
    format!(
        "{scope}\u{1}{id}\u{1}{fingerprint}\u{1}{}{organized}",
        shaped(optimize)
    )
}
/// How a book was shaped, as part of a key. The screen it was made for belongs
/// here: a copy optimized for one device must never be served from the cache
/// as if it had been optimized for another.
fn shaped(optimize: Option<crate::profile::Profile>) -> &'static str {
    optimize.map_or("none", |profile| profile.marker)
}
/// Size and modification time: what a local or mounted file can be checked for
/// without reading it.
pub fn file_fingerprint(metadata: &Meta) -> Option<String> {
    let modified = metadata.modified?.duration_since(UNIX_EPOCH).ok()?;
    Some(format!(
        "{}.{}.{:09}",
        metadata.len,
        modified.as_secs(),
        modified.subsec_nanos()
    ))
}
pub fn variant_key(
    sha: &str,
    optimize: Option<crate::profile::Profile>,
    organized: bool,
) -> String {
    format!("{sha}\u{1}{}{organized}", shaped(optimize))
}
impl Index {
    /// No store keeps nothing between scans; a file that cannot be read or
    /// parsed starts empty rather than failing.
    pub fn open(store: Option<Store>, clock: Arc<dyn Clock>) -> Self {
        let stored = store
            .as_ref()
            .and_then(|store| crate::files::read(&*store.dir, &store.file, 256 << 20).ok())
            .and_then(|bytes| serde_json::from_slice::<Stored>(&bytes).ok())
            .filter(|stored| stored.version == VERSION)
            .unwrap_or_default();
        Self {
            store,
            clock,
            state: Mutex::new(State {
                stored,
                dirty: false,
            }),
        }
    }
    pub fn source(&self, key: &str) -> Option<Source> {
        let mut state = self.state.lock().ok()?;
        let seen = self.now();
        let entry = state.stored.sources.get_mut(key)?;
        entry.seen = seen;
        Some(entry.value.clone())
    }
    pub fn put_source(&self, key: String, value: Source) {
        if let Ok(mut state) = self.state.lock() {
            let seen = self.now();
            state.stored.sources.insert(key, Aged { value, seen });
            state.dirty = true;
        }
    }
    pub fn variant(&self, key: &str) -> Option<Variant> {
        let mut state = self.state.lock().ok()?;
        let seen = self.now();
        let entry = state.stored.variants.get_mut(key)?;
        entry.seen = seen;
        Some(entry.value.clone())
    }
    pub fn put_variant(&self, key: String, value: Variant) {
        if let Ok(mut state) = self.state.lock() {
            let seen = self.now();
            state.stored.variants.insert(key, Aged { value, seen });
            state.dirty = true;
        }
    }
    fn now(&self) -> u64 {
        self.clock
            .now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default()
    }
    /// Write the index back, replacing it atomically. Nothing here can fail a
    /// scan: a cache that cannot be stored simply costs the next scan its work.
    pub fn save(&self) {
        let (Some(store), Ok(mut state)) = (self.store.as_ref(), self.state.lock()) else {
            return;
        };
        if !state.dirty {
            return;
        }
        let cutoff = self.now().saturating_sub(MAX_AGE);
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
        if store.dir.replace(&store.file, &bytes).is_ok() {
            state.dirty = false;
        }
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
