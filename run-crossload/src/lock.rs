// What the lock holds, and whether something new fits inside it. A lock can't be widened once it
// is applied, so the terminal interface checks the folders and devices new settings name against
// it before starting again with them.

use std::path::{Path, PathBuf};

use ambit_lockdown::Grant;

/// The folders where removable media are mounted (udisks, and the older `/media` layout), for the
/// terminal interface: a Kobo or a card plugged in while it runs appears below one of these.
/// Read-write below the user's own; read on the rest, which `detect_kobos` lists. A folder that
/// isn't there when the lock is applied isn't covered (udisks makes `/run/media/$USER` on the first
/// mount after boot).
pub fn media() -> Vec<Grant> {
    let mut grants = vec![];
    if let Some(user) = std::env::var_os("USER").filter(|u| !u.is_empty()) {
        for parent in ["/run/media", "/media"] {
            grants.push(Grant::ReadWrite(Path::new(parent).join(&user)));
        }
    }
    for root in ["/run/media", "/media", "/mnt", "/Volumes"] {
        grants.push(Grant::Read(root.into()));
    }
    grants
}

/// What a lock made of `grants` holds: Landlock skips a path that isn't there, and follows links
/// to where they lead. Taken just before the lock is applied.
pub fn held(grants: &[Grant]) -> Vec<Grant> {
    grants
        .iter()
        .filter_map(|grant| match grant {
            Grant::Read(path) => path.canonicalize().ok().map(Grant::Read),
            Grant::ReadWrite(path) => path.canonicalize().ok().map(Grant::ReadWrite),
            other => Some(other.clone()),
        })
        .collect()
}

/// The grants in `wanted` that `held` doesn't cover. A path is covered by a held folder above it
/// (or the path itself) with at least that access, whether or not it is there yet.
pub fn uncovered(held: &[Grant], wanted: &[Grant]) -> Vec<Grant> {
    let covered = |grant: &Grant| match grant {
        Grant::Read(path) => {
            let path = resolved(path);
            held.iter().any(|h| match h {
                Grant::Read(h) | Grant::ReadWrite(h) => path.starts_with(h),
                _ => false,
            })
        }
        Grant::ReadWrite(path) => {
            let path = resolved(path);
            held.iter()
                .any(|h| matches!(h, Grant::ReadWrite(h) if path.starts_with(h)))
        }
        other => held.contains(other),
    };
    let mut missing: Vec<Grant> = vec![];
    for grant in wanted {
        if !covered(grant) && !missing.contains(grant) {
            missing.push(grant.clone());
        }
    }
    missing
}

/// `path` with the part that exists resolved as Landlock sees it (links followed), and the rest
/// as written.
fn resolved(path: &Path) -> PathBuf {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut rest = vec![];
    let mut existing = path.as_path();
    loop {
        if let Ok(found) = existing.canonicalize() {
            return rest.iter().rev().fold(found, |path, part| path.join(part));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                existing = parent;
            }
            _ => return path,
        }
    }
}

/// One grant in words, for the user.
pub fn describe(grant: &Grant) -> String {
    match grant {
        Grant::Read(path) => format!("reading {}", path.display()),
        Grant::ReadWrite(path) => format!("writing to {}", path.display()),
        Grant::Connect(port) => format!("connecting to port {port}"),
        Grant::Listen(port) => format!("listening on port {port}"),
        Grant::Resolve => "looking up names".into(),
        Grant::Udp => "UDP".into(),
        Grant::Terminal => "the terminal".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_below_a_held_one_is_covered_even_before_it_exists() {
        let temp = tempfile::tempdir().unwrap();
        let media = temp.path().join("media");
        std::fs::create_dir(&media).unwrap();
        let held = held(&[
            Grant::ReadWrite(media.clone()),
            Grant::Read(temp.path().into()),
            Grant::ReadWrite(temp.path().join("not there yet")),
            Grant::Connect(80),
        ]);
        let wanted = [
            Grant::ReadWrite(media.join("KOBOeReader")),
            Grant::Read(temp.path().join("other")),
            Grant::ReadWrite(temp.path().join("other")),
            Grant::ReadWrite(temp.path().join("not there yet")),
            Grant::Connect(80),
            Grant::Connect(81),
        ];
        assert_eq!(
            uncovered(&held, &wanted),
            vec![
                Grant::ReadWrite(temp.path().join("other")),
                Grant::ReadWrite(temp.path().join("not there yet")),
                Grant::Connect(81),
            ]
        );
    }

    #[test]
    fn a_link_is_covered_where_it_leads() {
        let temp = tempfile::tempdir().unwrap();
        let books = temp.path().join("books");
        std::fs::create_dir(&books).unwrap();
        std::os::unix::fs::symlink(&books, temp.path().join("link")).unwrap();
        let held = held(&[Grant::ReadWrite(temp.path().join("link"))]);
        assert!(uncovered(&held, &[Grant::ReadWrite(books.join("new"))]).is_empty());
    }
}
