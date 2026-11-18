// Copyright (c) Michael Grier.

use std::cell::Cell;

use super::*;

/// A scratch directory removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "acadia-os-{}-{tag}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Scratch(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_taken_staging_name_is_skipped_and_left_alone() {
    let dir = Scratch::new("stale-stage");
    let target = dir.0.join("record");
    let stale = dir.0.join("record.publishing.stale");
    let fresh = dir.0.join("record.publishing.fresh");
    fs::write(&stale, b"left by a crash").unwrap();
    let mut names = [stale.clone(), fresh.clone()].into_iter();

    publish_staged(&target, b"published", || names.next()).unwrap();

    assert_eq!(fs::read(&target).unwrap(), b"published");
    assert_eq!(fs::read(&stale).unwrap(), b"left by a crash");
    assert!(!fresh.exists(), "the staging sibling used is removed");
}

#[test]
fn a_publication_whose_every_staging_name_is_taken_does_not_claim_the_target() {
    let dir = Scratch::new("all-stale");
    let target = dir.0.join("record");
    let stale = dir.0.join("record.publishing.stale");
    fs::write(&stale, b"left by a crash").unwrap();
    let tried = Cell::new(0);

    let error = publish_staged(&target, b"published", || {
        tried.set(tried.get() + 1);
        Some(stale.clone())
    })
    .unwrap_err();

    assert_eq!(error.kind(), VfsErrorKind::Other);
    assert_eq!(tried.get(), STAGING_ATTEMPTS);
    assert!(!target.exists());
    assert_eq!(fs::read(&stale).unwrap(), b"left by a crash");
}

#[test]
fn a_taken_target_still_refuses_after_a_taken_staging_name() {
    let dir = Scratch::new("stale-and-taken");
    let target = dir.0.join("record");
    let stale = dir.0.join("record.publishing.stale");
    let fresh = dir.0.join("record.publishing.fresh");
    fs::write(&target, b"first").unwrap();
    fs::write(&stale, b"left by a crash").unwrap();
    let mut names = [stale, fresh.clone()].into_iter();

    let error = publish_staged(&target, b"second", || names.next()).unwrap_err();

    assert_eq!(error.kind(), VfsErrorKind::AlreadyExists);
    assert_eq!(fs::read(&target).unwrap(), b"first");
    assert!(!fresh.exists(), "the staging sibling used is removed");
}

#[test]
fn a_path_naming_no_file_is_invalid_input() {
    let error =
        publish_staged(Path::new("/"), b"x", || publishing_path(Path::new("/"))).unwrap_err();
    assert_eq!(error.kind(), VfsErrorKind::InvalidInput);
}

#[test]
fn every_staging_name_a_publication_tries_is_recognised_as_its_own() {
    for target in ["record", ".store", "a.b", "publishing"] {
        let path = Path::new("/root").join(target);
        for _ in 0..10 {
            let staging = publishing_path(&path).unwrap();
            let name = staging.file_name().unwrap();
            assert!(
                crate::is_publication_staging_of(name, std::ffi::OsStr::new(target)),
                "{name:?}"
            );
            assert!(!crate::is_publication_staging_of(
                name,
                std::ffi::OsStr::new("other")
            ));
        }
    }
}

/// Windows refuses for a while to remove the last link of a file whose other link was removed
/// while an outside opener -- a scanner of new files -- still held it, so a publication is a
/// move there and leaves its file with one name.
#[test]
fn a_published_file_can_be_replaced_by_a_rename_at_once() {
    let dir = Scratch::new("replace-published");
    let fs_ = OsFs::new();
    for round in 0..40 {
        let target = dir.0.join(format!("record-{round}"));
        fs_.publish_new(&target, b"published").unwrap();
        let replacement = dir.0.join(format!("record-{round}.tmp"));
        fs::write(&replacement, b"replaced").unwrap();
        fs::rename(&replacement, &target).unwrap_or_else(|e| panic!("round {round}: {e}"));
        assert_eq!(fs::read(&target).unwrap(), b"replaced");
    }
}

#[test]
fn a_publication_racing_a_directory_of_its_name_refuses() {
    let dir = Scratch::new("directory-taken");
    let target = dir.0.join("record");
    fs::create_dir(&target).unwrap();
    let error = OsFs::new().publish_new(&target, b"x").unwrap_err();
    assert_eq!(error.kind(), VfsErrorKind::AlreadyExists);
    assert!(target.is_dir());
    assert_eq!(
        fs::read_dir(&dir.0).unwrap().count(),
        1,
        "no staging sibling is left"
    );
}
