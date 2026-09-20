//! Correcting what a book says about itself, from the library.
//!
//! The draft here is separate from the book until saving succeeds, as the
//! settings panel's is. Only a local EPUB can be corrected: a book that is only
//! on a device is not ours to rewrite, and a PDF or CBZ has no package document
//! to rewrite.
use crate::books::{Book, Place};
use anyhow::{ensure, Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::PathBuf;

pub(super) const LABELS: [&str; 5] = [
    "Title",
    "Author",
    "Series",
    "Position in the series",
    "Save and rescan",
];
const HINTS: [&str; 5] = [
    "What the book is called. Emptying it keeps the title it has.",
    "One spelling, for every book that shares an author.",
    "The series this belongs to, written both ways so any reader finds it.",
    "Its number in that series. 2 sorts before 10, and 1.5 between them.",
    "Rewrites the local copy. Only its metadata changes, so copies already on a\
     device still count as the same book.",
];

pub(super) struct Task {
    pub path: PathBuf,
    pub edit: crate::metadata::Edit,
}
pub(super) enum Action {
    None,
    Close,
    Run(Task),
}

pub(super) struct Panel {
    pub values: [String; 4],
    pub selected: usize,
    /// While editing, cursor is a byte boundary in the selected UTF-8 string.
    pub edit: Option<(String, usize)>,
    /// A complaint, shown in place of the selected row's hint.
    pub message: String,
    pub book: String,
    path: PathBuf,
    before: [String; 4],
}

impl Panel {
    /// A panel for a book, or the reason there cannot be one.
    pub fn new(book: &Book) -> Result<Self> {
        let copy = book
            .copies
            .iter()
            .find(|copy| copy.place == Place::Local && copy.format.rewritten())
            .context("Only a local EPUB can be corrected; copy it here first")?;
        ensure!(
            !copy.sha.is_empty(),
            "This copy could not be read during discovery; resolve that first"
        );
        let values = [
            book.title.clone(),
            book.author.clone(),
            book.series.clone().unwrap_or_default(),
            book.series_index
                .map(|index| {
                    if index.fract() == 0.0 {
                        format!("{}", index as i64)
                    } else {
                        format!("{index}")
                    }
                })
                .unwrap_or_default(),
        ];
        Ok(Self {
            before: values.clone(),
            values,
            selected: 0,
            edit: None,
            message: String::new(),
            book: book.title.clone(),
            path: PathBuf::from(&copy.path),
        })
    }

    pub fn hint(&self) -> String {
        if !self.message.is_empty() {
            return self.message.clone();
        }
        HINTS[self.selected.min(HINTS.len() - 1)].to_owned()
    }

    /// Only what the reader actually changed is written, so a field left alone
    /// cannot quietly rewrite itself into a different spelling.
    fn wanted(&self) -> crate::metadata::Edit {
        let changed = |index: usize| {
            (self.values[index].trim() != self.before[index].trim())
                .then(|| self.values[index].trim().to_owned())
        };
        crate::metadata::Edit {
            title: changed(0),
            author: changed(1),
            series: changed(2),
            series_index: changed(3).or_else(|| {
                // A series without a position keeps the position it had.
                changed(2).map(|_| self.values[3].trim().to_owned())
            }),
        }
    }

    fn save(&mut self) -> Action {
        let edit = self.wanted();
        if edit.is_empty() {
            self.message = "Nothing was changed.".into();
            return Action::None;
        }
        if self.values[0].trim().is_empty() {
            self.selected = 0;
            self.message = "A book needs a title.".into();
            return Action::None;
        }
        if !self.values[3].trim().is_empty() && self.values[3].trim().parse::<f32>().is_err() {
            self.selected = 3;
            self.message = "A position in a series has to be a number.".into();
            return Action::None;
        }
        Action::Run(Task {
            path: self.path.clone(),
            edit,
        })
    }

    pub fn input(&mut self, key: KeyEvent) -> Action {
        if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.edit = None;
            return self.save();
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
            KeyCode::Down | KeyCode::Tab => {
                self.selected = (self.selected + 1) % LABELS.len();
                self.message.clear();
            }
            KeyCode::Up | KeyCode::BackTab => {
                self.selected = (self.selected + LABELS.len() - 1) % LABELS.len();
                self.message.clear();
            }
            KeyCode::Enter | KeyCode::Char('e') if self.selected < 4 => {
                self.message.clear();
                self.edit = Some((
                    self.values[self.selected].clone(),
                    self.values[self.selected].len(),
                ));
            }
            KeyCode::Enter => return self.save(),
            _ => {}
        }
        Action::None
    }
}

/// Rewrite the book in place. The new bytes are written beside it and moved
/// over it, so a book is never half written.
pub(super) fn perform(task: Task) -> Result<String> {
    let data = std::fs::read(&task.path)?;
    let changed = crate::metadata::apply(&data, &task.edit)?;
    let parent = task
        .path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    use std::io::Write;
    file.write_all(&changed)?;
    file.as_file().sync_all()?;
    file.persist(&task.path)
        .context("Cannot replace the book with its corrected self")?;
    Ok(format!("Corrected {}", task.path.display()))
}
