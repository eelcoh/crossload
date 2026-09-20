use crossload::cache::{self, Index, Source, Variant};
use crossload::profile::X4;
use std::fs;
fn source(sha: &str) -> Source {
    Source {
        title: "Book".into(),
        author: "Author".into(),
        format: crossload::format::Format::Epub,
        verdict: None,
        series: None,
        series_index: None,
        size: 42,
        sha: sha.into(),
        resources: None,
        optimized: false,
        locked: false,
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
fn a_copy_made_for_one_screen_is_never_served_as_one_made_for_another() {
    use crossload::profile::Profile;
    // A second screen, standing in for whatever device is added next.
    const OTHER: Profile = Profile {
        device: "Other",
        width: 960,
        height: 540,
        marker: "other-images-v1:960x540:gray:jpeg85",
    };
    let sha = "abc123";
    let x4 = cache::variant_key(sha, Some(X4), true);
    assert_ne!(cache::variant_key(sha, Some(OTHER), true), x4);
    assert_ne!(cache::variant_key(sha, None, true), x4);
    assert_eq!(cache::variant_key(sha, Some(X4), true), x4);
    // The same must hold for the key a whole source is stored under.
    let source = cache::source_key("CrossPoint", "/b.epub", "1.0.0", Some(X4), true);
    assert_ne!(
        cache::source_key("CrossPoint", "/b.epub", "1.0.0", Some(OTHER), true),
        source
    );
}

#[test]
fn keys_separate_scopes_sources_fingerprints_and_options() {
    let key = cache::source_key("Local", "/book.epub", "5.1.0", Some(X4), true);
    assert_eq!(
        cache::source_key("Local", "/book.epub", "5.1.0", Some(X4), true),
        key
    );
    // Scope keeps two devices from sharing an identity, and the options that
    // shaped the stored variants are part of the key.
    assert_ne!(
        cache::source_key("Kobo", "/book.epub", "5.1.0", Some(X4), true),
        key
    );
    assert_ne!(
        cache::source_key("Local", "/other.epub", "5.1.0", Some(X4), true),
        key
    );
    assert_ne!(
        cache::source_key("Local", "/book.epub", "6.1.0", Some(X4), true),
        key
    );
    // A copy made for one screen must never be served from the cache as if it
    // had been made for another. This is the whole reason the key carries it.
    assert_ne!(
        cache::source_key("Local", "/book.epub", "5.1.0", None, true),
        key
    );
    assert_ne!(
        cache::source_key("Local", "/book.epub", "5.1.0", Some(X4), false),
        key
    );
    assert_eq!(
        cache::variant_key("sha", Some(X4), true),
        cache::variant_key("sha", Some(X4), true)
    );
    assert_ne!(
        cache::variant_key("sha", Some(X4), true),
        cache::variant_key("sha", None, true)
    );
}
#[test]
fn a_file_fingerprint_follows_size_and_modification_time() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("book.epub");
    fs::write(&path, b"bytes").unwrap();
    let first = cache::file_fingerprint(&path).unwrap();
    assert_eq!(cache::file_fingerprint(&path).unwrap(), first);
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    fs::write(&path, b"longer bytes").unwrap();
    let grown = cache::file_fingerprint(&path).unwrap();
    assert_ne!(grown, first);
    // Same length with a restored timestamp is deliberately indistinguishable.
    fs::write(&path, b"longer BYTES").unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    assert_ne!(cache::file_fingerprint(&path).unwrap(), grown);
    assert!(cache::file_fingerprint(&tmp.path().join("absent")).is_none());
}
