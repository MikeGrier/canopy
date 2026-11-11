// Copyright (c) Michael Grier.

//! A nonce that is exactly a Windows reserved device name commits over the real filesystem.
//!
//! The nonce names the transaction's pointer staging directory. On Windows such a name cannot
//! be a directory, so it is substituted there (`Nonce::path_component`); everywhere else it is
//! used as is. Only a real filesystem can show that the substitute is a name Windows accepts.

use std::path::PathBuf;
use std::sync::Arc;

use acadia::{OsFs, Vfs};
use canopy::{
    DocumentBody, DocumentName, GenesisConfig, NodePath, Nonce, Read, ShireId, Store, StoreOptions,
    Swap, SwapSet, TransactionOutcome,
};

/// The nonces the commit test draws, in order: every reserved device name, then `con`, which
/// folds to the first of them.
const RESERVED: [&str; 6] = ["CON", "prn", "Aux", "NUL", "COM1", "LPT9"];

/// A unique scratch directory on the real filesystem, removed on drop.
struct TempRoot(PathBuf);

impl TempRoot {
    fn new(tag: &str) -> TempRoot {
        let mut path = std::env::temp_dir();
        path.push(format!("canopy-reserved-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create scratch root");
        TempRoot(path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn transactions_named_by_reserved_device_names_commit_and_leave_no_staging() {
    let root = TempRoot::new("commit");
    let vfs: Arc<dyn Vfs> = Arc::new(OsFs::new());
    let config = GenesisConfig::new(ShireId::new("reserved-nonce").unwrap(), 0);
    let mut nonces = RESERVED
        .iter()
        .chain(&["con"])
        .map(|raw| Nonce::new(raw).unwrap())
        .collect::<Vec<_>>()
        .into_iter();
    let options = StoreOptions::with_nonce_source(move || nonces.next().unwrap());
    let store = Store::init(Arc::clone(&vfs), root.0.clone(), config, options).expect("init");
    let node = NodePath::parse("tickets").unwrap();

    for (index, raw) in RESERVED.iter().enumerate() {
        let document = DocumentName::new(&format!("doc{index}")).unwrap();
        let body = DocumentBody::new(format!("written under {raw}").into_bytes());
        let swaps = SwapSet::single(Swap::new(&node, document.clone(), body.clone()));
        let commit = store
            .apply_transaction(&swaps)
            .unwrap_or_else(|error| panic!("nonce {raw}: {error}"));
        assert_eq!(commit.nonce(), &Nonce::new(raw).unwrap());
        assert_eq!(
            commit.outcome(),
            &TransactionOutcome::Committed,
            "nonce {raw}"
        );
        assert_eq!(
            store.read_document(&node, &document).unwrap(),
            Read::Found(body)
        );
    }

    // `con` folds to `CON`, the same nonce. Each write emptied the log and released its claim,
    // so the id is free again, and the second write reuses the same staging name without
    // colliding with anything the first left behind.
    let document = DocumentName::new("doc0").unwrap();
    let body = DocumentBody::new(b"rewritten under con".to_vec());
    let swaps = SwapSet::single(Swap::new(&node, document.clone(), body.clone()));
    store.apply_transaction(&swaps).expect("lowercase con");
    assert_eq!(
        store.read_document(&node, &document).unwrap(),
        Read::Found(body)
    );

    drop(store);
    for entry in std::fs::read_dir(root.0.join("namespace")).unwrap() {
        let staging = entry.unwrap().path().join(".staging");
        if let Ok(left) = std::fs::read_dir(&staging) {
            assert_eq!(left.count(), 0, "staging left behind under {staging:?}");
        }
    }
}

/// A directory literally named with the substitute is not where a `CON` transaction stages
/// on a platform that does not substitute, and the two coexist on one that does not.
#[cfg(not(windows))]
#[test]
fn con_and_its_substitute_name_distinct_directories_off_windows() {
    let root = TempRoot::new("distinct");
    let staging = root.0.join("staging");
    std::fs::create_dir_all(staging.join("CON")).unwrap();
    std::fs::create_dir_all(staging.join("\u{00A9}ON")).unwrap();
    let names: Vec<String> = std::fs::read_dir(&staging)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert_eq!(Nonce::new("CON").unwrap().path_component(), "CON");
}

/// On Windows the substitute is a real, creatable directory name distinct from the device.
#[cfg(windows)]
#[test]
fn the_substitute_names_a_real_directory_on_windows() {
    let root = TempRoot::new("substitute");
    let component = Nonce::new("CON").unwrap().path_component().into_owned();
    assert_ne!(component, "CON");
    let dir = root.0.join(&component);
    std::fs::create_dir(&dir).expect("the substitute is a valid directory name");
    std::fs::write(dir.join("probe"), b"x").unwrap();
    assert_eq!(std::fs::read(dir.join("probe")).unwrap(), b"x");
    let names: Vec<String> = std::fs::read_dir(&root.0)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [component], "listed under its own name, not as CON");
}
