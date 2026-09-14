use crossload::cache::{self, Index, Source, Variant};
use std::fs;
fn source(sha: &str) -> Source {
    Source {
        title: "Book".into(),
        author: "Author".into(),
        sha: sha.into(),
        resources: None,
        optimized: false,
        variants: vec![(sha.into(), None)],
    }
}
#[test]
fn stored_identities_survive_reopening_and_bad_files_never_fail() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("nested/index.json");
    let index = Index::open(Some(path.clone()));
    assert!(index.source("missing").is_none());
    index.put_source("key".into(), source("abc"));
    index.put_variant(
        "variant".into(),
        Variant {
            sha: "def".into(),
            resources: Some("res".into()),
        },
    );
    index.save();
    let reopened = Index::open(Some(path.clone()));
    assert_eq!(reopened.source("key").unwrap().sha, "abc");
    assert_eq!(reopened.variant("variant").unwrap().sha, "def");
    // Corrupt content and a version from another release start empty rather
    // than failing, because a scan must never depend on the cache.
    fs::write(&path, b"not json at all").unwrap();
    assert!(Index::open(Some(path.clone())).source("key").is_none());
    fs::write(&path, br#"{"version":999,"sources":{},"variants":{}}"#).unwrap();
    assert!(Index::open(Some(path.clone())).source("key").is_none());
    // A location that cannot be written is silent, not an error.
    let blocked = tmp.path().join("occupied");
    fs::write(&blocked, b"x").unwrap();
    let index = Index::open(Some(blocked.join("index.json")));
    index.put_source("key".into(), source("abc"));
    index.save();
    assert_eq!(fs::read(&blocked).unwrap(), b"x");
}
#[test]
fn keys_change_with_size_timestamp_and_preparation_options() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("book.epub");
    fs::write(&path, b"bytes").unwrap();
    let (key, size) = cache::source_key("Local", &path, true, true).unwrap();
    assert_eq!(size, 5);
    assert_eq!(
        cache::source_key("Local", &path, true, true).unwrap().0,
        key
    );
    // The options that shaped the stored variants are part of the key.
    assert_ne!(
        cache::source_key("Local", &path, false, true).unwrap().0,
        key
    );
    assert_ne!(
        cache::source_key("Local", &path, true, false).unwrap().0,
        key
    );
    assert_ne!(cache::source_key("Kobo", &path, true, true).unwrap().0, key);
    // So are the file's own size and modification time.
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    fs::write(&path, b"longer bytes").unwrap();
    let (grown, size) = cache::source_key("Local", &path, true, true).unwrap();
    assert_eq!(size, 12);
    assert_ne!(grown, key);
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    assert_ne!(
        cache::source_key("Local", &path, true, true).unwrap().0,
        grown
    );
    assert!(cache::source_key("Local", &tmp.path().join("absent"), true, true).is_none());
    assert_eq!(
        cache::variant_key("sha", true, true),
        cache::variant_key("sha", true, true)
    );
    assert_ne!(
        cache::variant_key("sha", true, true),
        cache::variant_key("sha", false, true)
    );
}
