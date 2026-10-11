//! A folder that comes and goes: a Kobo or a reader's card, mounted at a known path.
//!
//! `NativeDir` opens its tree once, so a device plugged in after crossload started would never be
//! seen. This opens it again on every call: the tree is always whatever is mounted at the path
//! now, with the same rules (paths stay inside it, symlinks leading out are refused). A device
//! that is not there answers `NotFound`.
use std::path::{Path, PathBuf};

use ambit_caps_dir::{
    Dir, DirError, DirErrorKind, DirModify, DirWrite, Entry, File, Meta, RelPath,
};
use ambit_caps_dir_native::NativeDir;

pub struct Mount {
    path: PathBuf,
    /// Create the folder (and its parents) when something is first added to it.
    create: bool,
}

impl Mount {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            create: false,
        }
    }

    /// A folder of crossload's own choosing (the import folder): made when the first book is
    /// added to it, not before. Never used for a device: an unplugged card must not turn into a
    /// folder on this computer.
    pub fn creating(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            create: true,
        }
    }

    /// The tree, made first if this mount may and is about to be written to.
    fn writable(&self) -> Result<NativeDir, DirError> {
        if self.create && !self.path.exists() {
            std::fs::create_dir_all(&self.path).map_err(|e| {
                DirError::new(
                    DirErrorKind::Io,
                    format!("Cannot create {}: {e}", self.path.display()),
                )
            })?;
        }
        self.dir()
    }

    fn dir(&self) -> Result<NativeDir, DirError> {
        NativeDir::open(&self.path).map_err(|e| {
            DirError::new(
                DirErrorKind::NotFound,
                format!("{} is not mounted: {e}", self.path.display()),
            )
        })
    }
}

impl Dir for Mount {
    fn metadata(&self, path: &RelPath) -> Result<Meta, DirError> {
        self.dir()?.metadata(path)
    }
    fn open(&self, path: &RelPath) -> Result<Box<dyn File>, DirError> {
        self.dir()?.open(path)
    }
    fn list(&self, path: &RelPath) -> Result<Vec<Entry>, DirError> {
        self.dir()?.list(path)
    }
}

impl DirWrite for Mount {
    fn create_dir(&self, path: &RelPath) -> Result<(), DirError> {
        self.writable()?.create_dir(path)
    }
    fn create_new(&self, path: &RelPath, contents: &[u8]) -> Result<(), DirError> {
        self.writable()?.create_new(path, contents)
    }
    fn available_space(&self, path: &RelPath) -> Result<u64, DirError> {
        // A folder yet to be made has the room of the disk it will be made on.
        match (self.create, self.dir()) {
            (_, Ok(dir)) => dir.available_space(path),
            (true, Err(_)) => {
                let existing =
                    self.path.ancestors().find(|p| p.is_dir()).ok_or_else(|| {
                        DirError::new(DirErrorKind::NotFound, "no existing parent")
                    })?;
                NativeDir::open(existing)
                    .map_err(|e| DirError::new(DirErrorKind::NotFound, e.to_string()))?
                    .available_space(&RelPath::root())
            }
            (false, Err(e)) => Err(e),
        }
    }
}

impl DirModify for Mount {
    fn replace(&self, path: &RelPath, contents: &[u8]) -> Result<(), DirError> {
        self.dir()?.replace(path, contents)
    }
    fn remove_file(&self, path: &RelPath) -> Result<(), DirError> {
        self.dir()?.remove_file(path)
    }
}
