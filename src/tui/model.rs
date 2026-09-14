use super::{Entry, Options, Source};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
#[derive(Debug)]
pub(super) enum Message {
    Input(Event),
    InputError(String),
    Refresh,
    Tick,
    Loaded(u64, Result<Vec<Entry>, String>),
    Progress(u64, String),
    Finished(u64, Result<String, String>),
    Presence(u64, Result<Vec<(Source, String)>, String>),
}
pub(super) enum Effect {
    Load {
        id: u64,
        options: Box<Options>,
        local: bool,
    },
    Work {
        id: u64,
        options: Box<Options>,
        entry: Entry,
    },
    Quit,
    Check {
        id: u64,
        options: Box<Options>,
        entries: Vec<Entry>,
    },
}
pub(super) struct Model {
    pub options: Options,
    pub local: bool,
    pub entries: Vec<Entry>,
    pub query: String,
    pub selected: usize,
    pub search: bool,
    pub busy: Option<u64>,
    pub loading: Option<u64>,
    pub pending_quit: bool,
    pub status: String,
    pub tick: usize,
    pub checking: Option<u64>,
    pub presence: Vec<(Source, String)>,
    next_id: u64,
}
impl Model {
    pub fn new(options: Options) -> Self {
        Self {
            local: options.device.is_none(),
            options,
            entries: vec![],
            query: String::new(),
            selected: 0,
            search: false,
            busy: None,
            loading: None,
            pending_quit: false,
            status: "Loading books…".into(),
            tick: 0,
            checking: None,
            presence: vec![],
            next_id: 0,
        }
    }
    pub fn filtered(&self) -> Vec<&Entry> {
        let query = self.query.to_lowercase();
        self.entries
            .iter()
            .filter(|e| {
                format!("{} {}", e.title, e.author)
                    .to_lowercase()
                    .contains(&query)
            })
            .collect()
    }
    fn selected_entry(&self) -> Option<Entry> {
        self.filtered().get(self.selected).map(|e| (*e).clone())
    }
    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }
    fn reload(&mut self, clear: bool) -> Vec<Effect> {
        self.checking = None;
        self.presence.clear();
        if clear {
            self.entries.clear();
            self.query.clear();
            self.selected = 0;
        }
        let id = self.id();
        self.loading = Some(id);
        if self.busy.is_none() {
            self.status = "Loading books…".into();
        }
        vec![Effect::Load {
            id,
            options: Box::new(self.options.clone()),
            local: self.local,
        }]
    }
    fn quit(&mut self) -> Vec<Effect> {
        if self.busy.is_some() || self.loading.is_some() || self.checking.is_some() {
            self.pending_quit = true;
            self.status = "Will quit when the current work finishes.".into();
            vec![]
        } else {
            vec![Effect::Quit]
        }
    }
    pub fn update(&mut self, message: Message) -> Vec<Effect> {
        match message {
            Message::Refresh if !self.pending_quit => return self.reload(false),
            Message::Tick => self.tick = self.tick.wrapping_add(1),
            Message::Loaded(id, result) if self.loading == Some(id) => {
                self.loading = None;
                let selected = self.selected_entry().map(|e| e.source);
                match result {
                    Ok(entries) => {
                        self.entries = entries;
                        self.selected = selected
                            .and_then(|s| self.filtered().iter().position(|e| e.source == s))
                            .unwrap_or(0);
                        if self.busy.is_none() && !self.pending_quit {
                            self.status =
                                "Select a book. Enter opens, imports or transfers it.".into();
                            if self.options.send_to.is_some() || self.options.copy_to.is_some() {
                                let check_id = self.id();
                                self.checking = Some(check_id);
                                self.status =
                                    "Checking reader contents; browsing remains available…".into();
                                return vec![Effect::Check {
                                    id: check_id,
                                    options: Box::new(self.options.clone()),
                                    entries: self.entries.clone(),
                                }];
                            }
                        }
                    }
                    Err(error) => {
                        self.entries.clear();
                        self.selected = 0;
                        if self.busy.is_none() {
                            self.status = format!("Error: {error}");
                        }
                    }
                }
            }
            Message::Progress(id, progress) if self.busy == Some(id) => self.status = progress,
            Message::Finished(id, result) if self.busy == Some(id) => {
                self.busy = None;
                self.status = result.unwrap_or_else(|e| format!("Error: {e}"));
                self.presence.clear();
                self.status.push_str(" Press r to refresh reader status.");
            }
            Message::Presence(id, result) if self.checking == Some(id) => {
                self.checking = None;
                match result {
                    Ok(presence) => {
                        self.presence = presence;
                        self.status = "Reader checked. Missing means no matching contents; filename conflicts may still prevent transfer.".into();
                    }
                    Err(error) => {
                        self.presence.clear();
                        self.status = format!("Reader status unknown: {error}. Press r to retry.");
                    }
                }
            }
            Message::InputError(error) => {
                self.status = format!("Terminal error: {error}");
                return self.quit();
            }
            Message::Input(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    return self.quit();
                }
                if self.pending_quit {
                    return vec![];
                }
                if self.search {
                    match key.code {
                        KeyCode::Esc | KeyCode::Enter => self.search = false,
                        KeyCode::Backspace => {
                            self.query.pop();
                            self.selected = 0;
                        }
                        KeyCode::Char(c) => {
                            self.query.push(c);
                            self.selected = 0;
                        }
                        _ => {}
                    }
                    return vec![];
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return self.quit(),
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.selected =
                            (self.selected + 1).min(self.filtered().len().saturating_sub(1))
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.selected = self.selected.saturating_sub(1)
                    }
                    KeyCode::Char('/') => self.search = true,
                    KeyCode::Char('r') => return self.reload(false),
                    KeyCode::Char('o') => {
                        self.options.browse = self.options.output.clone();
                        self.local = true;
                        return self.reload(true);
                    }
                    KeyCode::Tab => {
                        if self.options.device.is_some() {
                            self.local = !self.local;
                            return self.reload(true);
                        }
                        self.status =
                            "Use --device <Kobo mount> to enable the Kobo browser.".into();
                    }
                    KeyCode::Backspace if self.local => {
                        if let Some(parent) = self.options.browse.parent() {
                            self.options.browse = parent.to_owned();
                            return self.reload(true);
                        }
                    }
                    KeyCode::Enter if self.loading.is_none() => {
                        if let Some(entry) = self.selected_entry() {
                            if let Source::Directory(path) = &entry.source {
                                self.options.browse = path.clone();
                                return self.reload(true);
                            }
                            if entry.preview {
                                self.status =
                                    "This is a preview. Download the full book on the Kobo first."
                                        .into();
                            } else if self.checking.is_some() {
                                self.status =
                                    "Wait for the reader check before transferring.".into();
                            } else if self.busy.is_none() {
                                let id = self.id();
                                self.busy = Some(id);
                                self.status = "Starting…".into();
                                return vec![Effect::Work {
                                    id,
                                    options: Box::new(self.options.clone()),
                                    entry,
                                }];
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        if self.pending_quit
            && self.busy.is_none()
            && self.loading.is_none()
            && self.checking.is_none()
        {
            return vec![Effect::Quit];
        }
        vec![]
    }
}
