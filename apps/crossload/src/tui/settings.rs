//! Settings drafts stay separate from the active library until saving succeeds.
use super::Options;
use crate::config::Defaults;
use ambit_tui::{Key, KeyCode};
use anyhow::{ensure, Result};

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
    "Optional. Enter lists the Kobos mounted when Crossload started; e types the path.",
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
fn expand(value: &str, home: Option<&str>) -> String {
    match (value.strip_prefix('~'), home) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') => {
            format!("{}{rest}", home.trim_end_matches('/'))
        }
        _ => value.to_owned(),
    }
}

pub(super) enum Task {
    Save { defaults: Defaults },
    Detect,
    Test { address: String, folder: String },
}
#[derive(Debug)]
pub(crate) enum Outcome {
    /// What was saved, normalized: what crossload should start again with.
    Saved(Defaults),
    Detected(Vec<String>),
    Tested(String),
}
pub(super) enum Action {
    None,
    Close,
    /// Boxed because a saved Task carries the whole of Defaults, and this is
    /// returned by value from every keystroke.
    Run(Box<Task>),
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
    pub devices: Vec<String>,
    pub device_selected: Option<usize>,
    /// The settings file, for the hint.
    path: String,
    home: Option<String>,
    /// The values Crossload started with, and whether each opened then.
    launched: [(String, Option<bool>); 6],
    /// Settings this panel does not show, carried through a save.
    kept: Defaults,
    /// The option each value came from, when the command line gave it.
    given: [Option<&'static str>; 6],
}
impl Panel {
    pub fn new(options: &Options, first_run: bool) -> Self {
        // Abbreviated, because the home directory is the least informative
        // part of a path on this screen.
        let home = options.home.clone();
        let display = |path: &str| {
            let rest = home.as_deref().and_then(|home| {
                let home = home.trim_end_matches('/');
                path.strip_prefix(home)
                    .filter(|rest| rest.is_empty() || rest.starts_with('/'))
            });
            match rest {
                Some("") | Some("/") => "~".to_owned(),
                Some(rest) => format!("~{rest}"),
                None => path.to_owned(),
            }
        };
        let values = [
            display(&options.browse),
            display(&options.output),
            options.device.as_deref().map(display).unwrap_or_default(),
            options.send_to.clone().unwrap_or_else(|| READER.to_owned()),
            options.copy_to.as_deref().map(display).unwrap_or_default(),
            options.folder.clone(),
        ];
        let launched = [
            (values[0].clone(), Some(options.opened.books)),
            (values[1].clone(), Some(options.opened.import)),
            (values[2].clone(), options.opened.kobo),
            (values[3].clone(), None),
            (values[4].clone(), options.opened.card),
            (values[5].clone(), None),
        ];
        let mut panel = Self {
            values,
            marks: [Mark::Silent; 6],
            selected: 0,
            edit: None,
            message: String::new(),
            first_run,
            devices: vec![],
            device_selected: None,
            path: options.config.clone(),
            home,
            launched,
            kept: options.defaults.clone(),
            given: options.given,
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
            7 => {
                let mut hint = format!(
                    "Writes these settings to {} and starts again with them.",
                    self.path
                );
                for row in 0..self.given.len() {
                    if let Some(note) = self.overridden(row) {
                        hint = format!("{hint} {note}");
                    }
                }
                hint
            }
            row => self
                .overridden(row)
                .unwrap_or_else(|| HINTS[row].to_owned()),
        }
    }
    /// For a value changed here that the command line also gave: that the command line's still
    /// counts once crossload starts again, since it starts with the same arguments.
    fn overridden(&self, row: usize) -> Option<String> {
        let option = (*self.given.get(row)?)?;
        (self.values[row] != self.launched[row].0)
            .then(|| format!("Started with {option}; that still applies after the restart."))
    }
    /// Judge one value on its own. Recomputed rather than read from `marks`, so
    /// validation never trusts a stale frame.
    fn mark(&self, index: usize) -> Mark {
        match self.judge(index) {
            Mark::Bad(reason) => Mark::Bad(reason),
            _ if self.overridden(index).is_some() => Mark::Warn("the command line's applies"),
            mark => mark,
        }
    }
    /// What the value itself says, before the command line has its say.
    fn judge(&self, index: usize) -> Mark {
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
        let path = expand(value, self.home.as_deref());
        if !path.starts_with('/') {
            return Mark::Bad("must start with / or ~/");
        }
        // Folders are opened by the wiring when Crossload starts, so what is
        // known about one is what was known then.
        let (launched, opened) = &self.launched[index];
        if self.values[index] != *launched {
            return match index {
                1 => Mark::Note("created on first import"),
                _ => Mark::Note("checked when saved"),
            };
        }
        match (index, opened) {
            (2, Some(true)) => Mark::Good("Kobo found"),
            (2, _) => Mark::Warn("no Kobo here now"),
            (4, Some(false) | None) => Mark::Warn("not mounted now"),
            (_, Some(true)) => Mark::Good("folder found"),
            (1, _) => Mark::Note("created on first import"),
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
        crate::crosspoint::Address::parse(address, "/").map_err(|e| (3, format!("{e:#}")))?;
        crate::crosspoint::Address::parse(address, &self.values[5])
            .map_err(|e| (5, format!("{e:#}")))?;
        let path = |index: usize| {
            (!self.values[index].is_empty()).then(|| self.values[index].trim().to_owned())
        };
        let mut defaults = Defaults {
            browse: path(0),
            output: path(1),
            device: path(2),
            reader: (!self.values[3].is_empty()).then(|| self.values[3].trim().to_owned()),
            copy_to: path(4),
            folder: Some(self.values[5].clone()),
            // Not offered on this panel; saved values are carried through.
            kepub: self.kept.kepub,
            profile: self.kept.profile.clone(),
            catalog: self.kept.catalog.clone(),
        };
        // Paths and the reader were checked above; ~ expansion is all that is
        // left to fail here, and it fails for the books folder first.
        defaults
            .normalize(self.home.as_deref())
            .map_err(|e| (0, format!("{e:#}")))?;
        Ok(defaults)
    }
    /// Take a detection result: fill it in, offer the choice, or say what the
    /// next move is. Detection answers the Kobo row, so it reports there.
    pub fn detected(&mut self, devices: Vec<String>) {
        self.selected = 2;
        self.message = match devices.len() {
            0 => "No Kobo was mounted when Crossload started. Connect it in USB mode, or press e to type its path.".to_owned(),
            1 => {
                self.values[2] = devices[0].clone();
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
            Ok(defaults) => Action::Run(Box::new(Task::Save { defaults })),
            Err((index, message)) => {
                self.edit = None;
                self.selected = index;
                self.message = message;
                Action::None
            }
        }
    }
    pub fn input(&mut self, key: Key) -> Action {
        // Ctrl+S is the shortcut past eight Tabs to "Save and rescan".
        if self.device_selected.is_none() && key.code == KeyCode::Char('s') && key.mods.ctrl {
            let action = self.save();
            self.check();
            return action;
        }
        let action = self.dispatch(key);
        self.check();
        action
    }
    fn dispatch(&mut self, key: Key) -> Action {
        if let Some(selected) = &mut self.device_selected {
            match key.code {
                KeyCode::Down => {
                    *selected = (*selected + 1).min(self.devices.len().saturating_sub(1))
                }
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Enter => {
                    self.values[2] = self.devices[*selected].clone();
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
                KeyCode::Char('u') if key.mods.ctrl => {
                    value.clear();
                    *cursor = 0;
                }
                KeyCode::Char(c) if !key.mods.ctrl && !key.mods.alt && !c.is_control() => {
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
            KeyCode::Enter if self.selected == 2 => return Action::Run(Box::new(Task::Detect)),
            KeyCode::Enter | KeyCode::Char('e') if self.selected < 6 => {
                self.message.clear();
                self.edit = Some((
                    self.values[self.selected].clone(),
                    self.values[self.selected].len(),
                ));
            }
            KeyCode::Enter if self.selected == 6 => {
                return Action::Run(Box::new(Task::Test {
                    address: self.values[3].clone(),
                    folder: self.values[5].clone(),
                }))
            }
            KeyCode::Enter => return self.save(),
            _ => {}
        }
        Action::None
    }
}

/// What the settings tasks may use.
pub(super) struct Context<'a> {
    pub store: Option<&'a crate::config::Store>,
    /// Kobos the wiring found mounted at launch.
    pub kobos: &'a [String],
    /// How this session reaches a reader, if it may.
    pub link: Option<&'a crate::crosspoint::Link>,
}

pub(super) fn perform(task: Task, context: Context<'_>) -> Result<Outcome> {
    match task {
        Task::Save { mut defaults } => {
            let store = context
                .store
                .ok_or_else(|| anyhow::anyhow!("This session has no settings file to save to"))?;
            defaults.save(store)?;
            Ok(Outcome::Saved(defaults))
        }
        Task::Detect => Ok(Outcome::Detected(context.kobos.to_vec())),
        Task::Test { address, folder } => {
            ensure!(
                !address.is_empty(),
                "Enter a reader address first (for example crosspoint.local)"
            );
            let link = context
                .link
                .ok_or_else(|| anyhow::anyhow!("This session may not reach a reader"))?;
            let reader = crate::crosspoint::Reader::new(
                crate::crosspoint::Address::parse(&address, &folder)?,
                link.clone(),
            );
            let status = reader.status()?;
            Ok(Outcome::Tested(format!(
                "Connected to {} at {address} · CrossPoint {}",
                status.device, status.version
            )))
        }
    }
}
