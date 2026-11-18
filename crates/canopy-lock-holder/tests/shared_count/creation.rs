// Copyright (c) Michael Grier.

//! Creators of one store in separate processes, released together (canopy's DESIGN-NOTES.md
//! "Store creation is idempotent").

use super::*;

/// How many creator processes race in each round.
const CREATORS: usize = 6;

/// Options for reading here through the coordination `mode` names, as the creators used it.
fn reading(mode: &str) -> StoreOptions {
    let options = StoreOptions::without_nonce_source(canopy::Agent::new(
        "canopy-tests",
        "a reader's version",
        "Canopy test fixture",
        1,
    ));
    if mode.starts_with("shared-") {
        options.with_coordination(Arc::new(acadia::coord::shared::SharedFactory::new()))
    } else {
        options
    }
}

/// Spawns creators of one identity, each with its own agent version and creation time, releases
/// them together once every one is ready, and waits for each to report its write.
fn create_together(root: &Path, mode: &str, round: usize) -> Vec<Writer> {
    let mut creators: Vec<Writer> = (0..CREATORS)
        .map(|i| {
            Writer::spawn_version(
                root,
                "creations",
                &format!("by-{round}-{i}"),
                &format!("written by creator {i}"),
                mode,
                &format!("version-{i}"),
            )
        })
        .collect();
    for creator in &creators {
        creator.expect("ready");
    }
    for creator in &mut creators {
        creator.go();
    }
    for creator in &creators {
        creator.expect("opened");
        creator.expect("wrote");
    }
    creators
}

#[test]
fn creators_in_separate_processes_make_one_store_and_none_loses_anothers_write() {
    for mode in ["create", "shared-create"] {
        for round in 0..3 {
            let root = TempRoot::new("creation");
            for creator in create_together(root.path(), mode, round) {
                creator.finish();
            }
            let reader = Store::open_read_only(vfs(), root.path(), reading(mode)).unwrap();
            assert_eq!(reader.store_id(), "cross-process-creation");
            for i in 0..CREATORS {
                assert_eq!(
                    reader
                        .read_document(node("creations"), &document(&format!("by-{round}-{i}")))
                        .unwrap(),
                    Read::Found(body(&format!("written by creator {i}"))),
                    "{mode} round {round}: creator {i}"
                );
            }
        }
    }
}

#[test]
fn a_creator_in_another_process_finds_a_store_made_here_and_keeps_its_documents() {
    let root = TempRoot::new("creation-found");
    let config = GenesisConfig::new(StoreId::new("cross-process-creation").unwrap(), 1);
    let made = Store::init(vfs(), root.path(), config, nonce_source("made-here")).unwrap();
    made.write_document_replicated(node("here"), &document("first"), &body("made here"))
        .unwrap()
        .agent_stamp
        .unwrap();
    drop(made);
    let mut creator = Writer::spawn_version(
        root.path(),
        "there",
        "second",
        "made there",
        "create",
        "a later version",
    );
    creator.expect("ready");
    creator.go();
    creator.expect("opened");
    creator.expect("wrote");
    creator.finish();
    let reader = open_reader(root.path());
    assert_eq!(
        reader
            .read_document(node("here"), &document("first"))
            .unwrap(),
        Read::Found(body("made here"))
    );
    assert_eq!(
        reader
            .read_document(node("there"), &document("second"))
            .unwrap(),
        Read::Found(body("made there"))
    );
}
