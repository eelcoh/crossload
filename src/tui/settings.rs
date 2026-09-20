//! Settings drafts stay separate from the active library until saving succeeds.
use super::Options;
use crate::config::Defaults;
use anyhow::{ensure, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::{Path, PathBuf};

/// The two folders Crossload cannot work without carry a star; everything else
/// says "Optional" in its hint, so nobody has to guess what a save needs.
pub(super) const LABELS: [&str; 8] = [
    "Books folder *",
    "Import folder *",
    "Kobo mount",
    "Reader address",
    "Reader card mount",
    "Reader base folder",
    "Test reader connection",
    "Save and rescan",
];
/// One line per row, shown while nothing more urgent has been reported. The
/// labels are short enough to fit; these say what they actually mean.
const HINTS: [&str; 8] = [
    "Required. Where your own books live. Crossload only ever reads from here.",
    "Required. Where imported and copied-back books are written; created on first import.",
    "Optional. Enter looks for a mounted Kobo; e types the path in yourself.",
    "Optional. The address the reader shows in CrossPoint File Transfer mode.",
    "Optional. A mounted reader card. When set, copies use the card instead of Wi-Fi.",
    "The folder on the reader that books go into; / is the reader's own root.",
    "Asks the reader for its status over Wi-Fi. Nothing is copied.",
    "",
];
/// The name CrossPoint announces itself under, and so the address to try first.
pub(super) const READER: &str = "crosspoint.local";

/// What a path field points at right now, so a typo shows where it is made
/// rather than at the end of the form.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Mark {
    Silent,
    Good(&'static str),
    Note(&'static str),
    Warn(&'static str),
    Bad(&'static str),
}
impl Mark {
    /// Glyph and word; colour is the view's business.
    pub fn parts(self) -> (&'static str, &'static str) {
        match self {
            Self::Silent => ("", ""),
            Self::Good(word) => ("✓", word),
            Self::Note(word) => ("·", word),
            Self::Warn(word) => ("⚠", word),
            Self::Bad(word) => ("✗", word),
        }
    }
}
/// Expand a leading ~ the way the configuration does, without rewriting the
/// draft the user is still editing.
fn expand(value: &str) -> PathBuf {
    match value.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(rest.trim_start_matches('/')))
            .unwrap_or_else(|| PathBuf::from(value)),
        _ => PathBuf::from(value),
    }
}

pub(super) enum Task {
    Save { path: PathBuf, defaults: Defaults },
    Detect,
    Test { address: String, folder: String },
}
#[derive(Debug)]
pub(super) enum Outcome {
    Saved(Defaults),
    Detected(Vec<PathBuf>),
    Tested(String),
}
pub(super) enum Action {
    None,
    Close,
    Run(Task),
}
pub(super) struct Panel {
    pub values: [String; 6],
    /// What each value points at, refreshed after every keystroke.
    pub marks: [Mark; 6],
    pub selected: usize,
    /// While editing, cursor is a byte boundary in the selected UTF-8 string.
    pub edit: Option<(String, usize)>,
    /// A result or a complaint. While it is empty the selected row's own hint
    /// takes its place, so the line always says something useful.
    pub message: String,
    pub first_run: bool,
    pub devices: Vec<PathBuf>,
    pub device_selected: Option<usize>,
    path: PathBuf,
}
impl Panel {
    pub fn new(options: &Options, first_run: bool) -> Self {
        // Absolute, because a saved path must be; abbreviated, because the home
        // directory is the least informative part of a path on this screen.
        let display = |path: &Path| {
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir().unwrap_or_default().join(path)
            };
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|home| home.is_absolute());
            match home.and_then(|home| path.strip_prefix(home).map(Path::to_path_buf).ok()) {
                Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
                Some(rest) => format!("~/{}", rest.display()),
                None => path.display().to_string(),
            }
        };
        let mut panel = Self {
            values: [
                display(&options.browse),
                display(&options.output),
                options.device.as_deref().map(display).unwrap_or_default(),
                options.send_to.clone().unwrap_or_else(|| READER.to_owned()),
                options.copy_to.as_deref().map(display).unwrap_or_default(),
                options.folder.clone(),
            ],
            marks: [Mark::Silent; 6],
            selected: 0,
            edit: None,
            message: String::new(),
            first_run,
            devices: vec![],
            device_selected: None,
            path: options.config.clone(),
        };
        panel.check();
        panel
    }
    /// The line under the fields: whatever was last reported, or else what the
    /// selected row is for.
    pub fn hint(&self) -> String {
        if !self.message.is_empty() {
            return self.message.clone();
        }
        match self.selected {
            7 => format!(
                "Writes these settings to {} and scans the new locations.",
                self.path.display()
            ),
            row => HINTS[row].to_owned(),
        }
    }
    /// Judge one value on its own. Recomputed rather than read from `marks`, so
    /// validation never trusts a stale frame.
    fn mark(&self, index: usize) -> Mark {
        let value = self.values[index].trim();
        if value.is_empty() {
            return match index {
                0 | 1 => Mark::Bad("not set"),
                _ => Mark::Silent,
            };
        }
        if index == 3 || index == 5 {
            return Mark::Silent;
        }
        let path = expand(value);
        if !path.is_absolute() {
            return Mark::Bad("must start with / or ~/");
        }
        match index {
            // The import folder is the one place Crossload may create itself.
            1 if !path.exists() => Mark::Note("created on first import"),
            2 if path.join(".kobo/KoboReader.sqlite").is_file() => Mark::Good("Kobo found"),
            2 if path.is_dir() => Mark::Warn("no Kobo database here"),
            _ if path.is_dir() => Mark::Good("folder found"),
            _ if path.exists() => Mark::Bad("not a folder"),
            _ => Mark::Bad("not found"),
        }
    }
    fn check(&mut self) {
        for index in 0..self.marks.len() {
            self.marks[index] = self.mark(index);
        }
    }
    /// The draft as settings, or the row to send the user back to and why.
    pub fn defaults(&self) -> std::result::Result<Defaults, (usize, String)> {
        let complain = |index: usize, reason: &str| {
            (
                index,
                format!("{}: {reason}", LABELS[index].trim_end_matches(" *")),
            )
        };
        for index in [0, 1, 2, 4] {
            if let Mark::Bad(reason) = self.mark(index) {
                return Err(complain(index, reason));
            }
        }
        // Address and folder are rejected by the same constructor, so ask it
        // about the address alone first to know which row is at fault.
        let address = match self.values[3].trim() {
            "" => READER,
            address => address,
        };
        crate::crosspoint::Reader::new(address, "/").map_err(|e| (3, format!("{e:#}")))?;
        crate::crosspoint::Reader::new(address, &self.values[5])
            .map_err(|e| (5, format!("{e:#}")))?;
        let path = |index: usize| {
            (!self.values[index].is_empty()).then(|| PathBuf::from(self.values[index].trim()))
        };
        let mut defaults = Defaults {
            browse: path(0),
            output: path(1),
            device: path(2),
            reader: (!self.values[3].is_empty()).then(|| self.values[3].trim().to_owned()),
            copy_to: path(4),
            folder: Some(self.values[5].clone()),
            // Not offered on this panel; the saved value is carried through.
            kepub: None,
        };
        // Paths and the reader were checked above; ~ expansion is all that is
        // left to fail here, and it fails for the books folder first.
        defaults.normalize().map_err(|e| (0, format!("{e:#}")))?;
        Ok(defaults)
    }
    /// Take a detection result: fill it in, offer the choice, or say what the
    /// next move is. Detection answers the Kobo row, so it reports there.
    pub fn detected(&mut self, devices: Vec<PathBuf>) {
        self.selected = 2;
        self.message = match devices.len() {
            0 => "No mounted Kobo found. Connect it in USB mode and press Enter again, or press e to type its path.".to_owned(),
            1 => {
                self.values[2] = devices[0].display().to_string();
                "Kobo found. Press e to change it, or save to use this mount.".to_owned()
            }
            _ => {
                self.device_selected = Some(0);
                "Choose a mounted Kobo with arrows and Enter.".to_owned()
            }
        };
        self.devices = devices;
        self.check();
    }
    /// Save from wherever the user is, or send them to the row that stopped it.
    fn save(&mut self) -> Action {
        match self.defaults() {
            Ok(defaults) => Action::Run(Task::Save {
                path: self.path.clone(),
                defaults,
            }),
            Err((index, message)) => {
                self.edit = None;
                self.selected = index;
                self.message = message;
                Action::None
            }
        }
    }
    pub fn input(&mut self, key: KeyEvent) -> Action {
        // Ctrl+S is the shortcut past eight Tabs to "Save and rescan".
        if self.device_selected.is_none()
            && key.code == KeyCode::Char('s')
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            let action = self.save();
            self.check();
            return action;
        }
        let action = self.dispatch(key);
        self.check();
        action
    }
    fn dispatch(&mut self, key: KeyEvent) -> Action {
        if let Some(selected) = &mut self.device_selected {
            match key.code {
                KeyCode::Down => {
                    *selected = (*selected + 1).min(self.devices.len().saturating_sub(1))
                }
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Enter => {
                    self.values[2] = self.devices[*selected].display().to_string();
                    self.device_selected = None;
                    self.message = "Kobo selected. Save to use this mount.".into();
                }
                KeyCode::Esc => self.device_selected = None,
                _ => {}
            }
            return Action::None;
        }
        if let Some((original, cursor)) = &mut self.edit {
            let value = &mut self.values[self.selected];
            match key.code {
                KeyCode::Esc => {
                    *value = original.clone();
                    self.edit = None;
                }
                KeyCode::Enter | KeyCode::Tab => {
                    self.edit = None;
                    self.selected = (self.selected + 1) % LABELS.len();
                    self.message.clear();
                }
                KeyCode::Home => *cursor = 0,
                KeyCode::End => *cursor = value.len(),
                KeyCode::Left => {
                    *cursor = value[..*cursor].char_indices().last().map_or(0, |(i, _)| i)
                }
                KeyCode::Right => {
                    *cursor += value[*cursor..].chars().next().map_or(0, char::len_utf8)
                }
                KeyCode::Backspace if *cursor > 0 => {
                    let previous = value[..*cursor].char_indices().last().unwrap().0;
                    value.drain(previous..*cursor);
                    *cursor = previous;
                }
                KeyCode::Delete if *cursor < value.len() => {
                    value.remove(*cursor);
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    value.clear();
                    *cursor = 0;
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                        && !c.is_control() =>
                {
                    value.insert(*cursor, c);
                    *cursor += c.len_utf8();
                }
                _ => {}
            }
            return Action::None;
        }
        match key.code {
            KeyCode::Esc => return Action::Close,
            // Moving is how a hint is asked for, so a stale result gives way.
            KeyCode::Down | KeyCode::Tab => {
                self.selected = (self.selected + 1) % LABELS.len();
                self.message.clear();
            }
            KeyCode::Up | KeyCode::BackTab => {
                self.selected = (self.selected + LABELS.len() - 1) % LABELS.len();
                self.message.clear();
            }
            // Enter does whatever the row is for. The Kobo mount is the one
            // value Crossload can find on its own, so asking for it beats
            // typing a path that only a mount table knows.
            KeyCode::Enter if self.selected == 2 => return Action::Run(Task::Detect),
            KeyCode::Enter | KeyCode::Char('e') if self.selected < 6 => {
                self.message.clear();
                self.edit = Some((
                    self.values[self.selected].clone(),
                    self.values[self.selected].len(),
                ));
            }
            KeyCode::Enter if self.selected == 6 => {
                return Action::Run(Task::Test {
                    address: self.values[3].clone(),
                    folder: self.values[5].clone(),
                })
            }
            KeyCode::Enter => return self.save(),
            _ => {}
        }
        Action::None
    }
}

/// Inspect conventional mount locations, never recurse into a device's books.
fn detect_in(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut devices = vec![];
    for root in roots {
        let candidates = std::iter::once(root.clone()).chain(
            std::fs::read_dir(root)
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok().map(|e| e.path())),
        );
        for candidate in candidates {
            if candidate.join(".kobo/KoboReader.sqlite").is_file() {
                let candidate = candidate.canonicalize().unwrap_or(candidate);
                if !devices.contains(&candidate) {
                    devices.push(candidate);
                }
            }
        }
    }
    devices.sort();
    devices
}
pub(super) fn perform(task: Task) -> Result<Outcome> {
    match task {
        Task::Save { path, mut defaults } => {
            defaults.save(&path)?;
            Ok(Outcome::Saved(defaults))
        }
        Task::Detect => {
            let mut roots = vec![
                PathBuf::from("/Volumes"),
                PathBuf::from("/mnt"),
                PathBuf::from("/media"),
            ];
            // Linux desktop mounts normally sit one level below the user directory.
            for parent in ["/run/media", "/media"] {
                roots.extend(
                    std::fs::read_dir(parent)
                        .into_iter()
                        .flatten()
                        .filter_map(|e| e.ok().map(|e| e.path())),
                );
            }
            Ok(Outcome::Detected(detect_in(&roots)))
        }
        Task::Test { address, folder } => {
            ensure!(
                !address.is_empty(),
                "Enter a reader address first (for example crosspoint.local)"
            );
            let reader = crate::crosspoint::Reader::new(&address, &folder)?;
            let status = reader.status()?;
            Ok(Outcome::Tested(format!(
                "Connected to {} at {address} · CrossPoint {}",
                status.device, status.version
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finds_kobo_mounts_without_guessing_from_volume_names() {
        let temp = tempfile::tempdir().unwrap();
        let mount = temp.path().join("My reader");
        std::fs::create_dir_all(mount.join(".kobo")).unwrap();
        std::fs::write(mount.join(".kobo/KoboReader.sqlite"), b"fixture").unwrap();
        std::fs::create_dir(temp.path().join("KOBOeReader")).unwrap();
        assert_eq!(
            detect_in(&[temp.path().into(), mount.clone()]),
            vec![mount.canonicalize().unwrap()]
        );
    }

    #[test]
    fn connection_test_requires_crosspoint_status_and_never_uploads() {
        use std::io::{Read, Write};
        for (body, success) in [
            (r#"{"version":"1.6.0","device":"X4"}"#, true),
            (r#"{"version":"1.6.0","device":"unknown"}"#, false),
            ("not JSON", false),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap().to_string();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                    .unwrap();
                let mut request = vec![];
                let mut buffer = [0; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = socket.read(&mut buffer).unwrap();
                    assert!(n > 0);
                    request.extend_from_slice(&buffer[..n]);
                }
                assert!(request.starts_with(b"GET /api/status HTTP/1.1\r\n"));
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            });
            let result = perform(Task::Test {
                address,
                folder: "/".into(),
            });
            server.join().unwrap();
            assert_eq!(result.is_ok(), success, "{result:?}");
        }
    }
}
