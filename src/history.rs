//! What was copied, when, and whether it arrived.
//!
//! Every copy is verified and nothing is ever overwritten, but until now
//! nothing remembered any of it. This is the record: append-only, one line per
//! book, readable by eye and by machine.
//!
//! Writing it is a courtesy, never a condition. A copy that succeeded must not
//! be reported as failed because a log line could not be written, so every
//! failure here is swallowed — exactly as the identity cache treats its own.
use crate::books::Place;
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, Write},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

/// One copy, as it happened.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Entry {
    /// Seconds since the epoch, so the file does not depend on a time zone.
    pub at: u64,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
    pub from: Place,
    pub to: Place,
    #[serde(default)]
    pub bytes: u64,
    /// Whether the book arrived. A failure is kept: it is the half of the
    /// record you go looking for.
    pub ok: bool,
    /// What was reported, whether that was where it landed or what went wrong.
    #[serde(default)]
    pub detail: String,
}

/// Keep a long tail, but not an unbounded one. Older lines are dropped from the
/// front when the file passes this.
const MAX_ENTRIES: usize = 5_000;

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

pub fn default_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    // A record is not a cache: it is not disposable, so it does not live with
    // the things that are.
    let base = if cfg!(target_os = "macos") {
        PathBuf::from(home).join("Library/Application Support")
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| PathBuf::from(home).join(".local/state"))
    };
    Some(base.join("crossload").join("history.jsonl"))
}

/// Civil date and time from a count of seconds, in UTC. Written out rather than
/// pulled in: this is the only date arithmetic in the program.
pub fn stamp(at: u64) -> String {
    let (days, rest) = ((at / 86_400) as i64, at % 86_400);
    // Days from the civil epoch, by Howard Hinnant's algorithm.
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        rest / 3600,
        (rest % 3600) / 60
    )
}

fn read_lines(path: &std::path::Path) -> Vec<String> {
    let Ok(file) = std::fs::File::open(path) else {
        return vec![];
    };
    std::io::BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter(|line| !line.trim().is_empty())
        .collect()
}

/// Add one copy to the record. Never fails a copy that already happened.
pub fn record(path: Option<PathBuf>, entry: &Entry) {
    let Some(path) = path.or_else(default_path) else {
        return;
    };
    let Ok(line) = serde_json::to_string(entry) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Trimming reads the file back, so it is done only when it has grown past
    // the limit rather than on every copy.
    let lines = read_lines(&path);
    if lines.len() >= MAX_ENTRIES {
        let keep: Vec<&String> = lines.iter().skip(lines.len() + 1 - MAX_ENTRIES).collect();
        let mut kept = keep.iter().fold(String::new(), |mut all, line| {
            all.push_str(line);
            all.push('\n');
            all
        });
        kept.push_str(&line);
        kept.push('\n');
        let _ = std::fs::write(&path, kept);
        return;
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "{line}");
    }
}

/// The most recent entries, newest last, as the file reads.
pub fn read(path: Option<PathBuf>, limit: usize) -> Vec<Entry> {
    let Some(path) = path.or_else(default_path) else {
        return vec![];
    };
    let lines = read_lines(&path);
    lines
        .iter()
        .skip(lines.len().saturating_sub(limit))
        // A line that cannot be parsed is skipped, not fatal: a record that
        // refuses to be read at all would be worse than one with a hole.
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(at: u64, title: &str, ok: bool) -> Entry {
        Entry {
            at,
            title: title.into(),
            author: "Frank Herbert".into(),
            from: Place::Local,
            to: Place::CrossPoint,
            bytes: 1024,
            ok,
            detail: "somewhere".into(),
        }
    }

    #[test]
    fn a_copy_is_remembered_with_what_happened_to_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/history.jsonl");
        assert!(read(Some(path.clone()), 10).is_empty());
        record(Some(path.clone()), &entry(1, "Dune", true));
        record(Some(path.clone()), &entry(2, "Piranesi", false));
        let entries = read(Some(path.clone()), 10);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].title, "Piranesi");
        // A failure is kept: it is the half you go looking for.
        assert!(!entries[1].ok && entries[0].ok);
        // Only the most recent are asked for.
        assert_eq!(read(Some(path.clone()), 1)[0].title, "Piranesi");

        // A line nothing can parse loses itself, not the record around it.
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("this is not JSON\n");
        std::fs::write(&path, text).unwrap();
        record(Some(path.clone()), &entry(3, "Neuromancer", true));
        let entries = read(Some(path), 10);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[2].title, "Neuromancer");
    }

    #[test]
    fn a_time_reads_as_a_date_without_a_calendar_crate() {
        assert_eq!(stamp(0), "1970-01-01 00:00");
        assert_eq!(stamp(1_000_000_000), "2001-09-09 01:46");
        // A leap day, and the turn of a century that is not a leap year.
        assert_eq!(stamp(1_709_164_800), "2024-02-29 00:00");
        assert_eq!(stamp(951_782_400), "2000-02-29 00:00");
    }

    #[test]
    fn nothing_here_can_fail_a_copy_that_already_happened() {
        // A path that cannot be written to is not an error, only a lost line.
        record(
            Some(PathBuf::from("/proc/nope/history.jsonl")),
            &entry(1, "Dune", true),
        );
        assert!(read(Some(PathBuf::from("/proc/nope/history.jsonl")), 10).is_empty());
    }
}
