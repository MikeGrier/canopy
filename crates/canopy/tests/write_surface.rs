// Copyright (c) Michael Grier.

//! Actors racing on one store through the write surface: writers move two documents together
//! in one planned transaction and readers take optimistic snapshots of both.
//!
//! The nonce generator is seeded from the operating system's entropy source and the seed is
//! printed, so a failing run's nonces can be reproduced. Scheduling cannot be reproduced from a
//! seed, so every actor also records what it did, in order, and a failure prints that trace.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

use acadia::{MemFs, Vfs};
use canopy::{
    DocumentBody, DocumentName, GenesisConfig, NodePath, Nonce, ShireId, Store, StoreOptions, Swap,
    SwapSet,
};

const WRITERS: u64 = 4;
const WRITES_EACH: u64 = 60;
const READERS: u64 = 2;
const SNAPSHOTS_EACH: u64 = 150;

/// SplitMix64 (Steele, Lea and Flood, 2014), the generator the unit tests also use; nonces are
/// rendered as 32 hex digits so the test needs nothing private from the crate.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

fn entropy_seed() -> u64 {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).expect("the operating system supplies entropy");
    u64::from_le_bytes(bytes)
}

fn node(raw: &str) -> NodePath {
    NodePath::parse(raw).unwrap()
}

fn doc(raw: &str) -> DocumentName {
    DocumentName::new(raw).unwrap()
}

fn value(body: Option<DocumentBody>) -> u64 {
    body.map(|body| {
        std::str::from_utf8(body.as_bytes())
            .unwrap()
            .parse()
            .unwrap()
    })
    .unwrap_or(0)
}

#[test]
fn planned_pairs_are_never_seen_torn_and_no_increment_is_lost() {
    let seed = entropy_seed();
    println!("nonce seed: {seed:#018x}");

    let vfs: Arc<dyn Vfs> = Arc::new(MemFs::new());
    let root = PathBuf::from("racing");
    vfs.create_dir_all(&root).unwrap();
    let mut generator = SplitMix64(seed);
    let options = StoreOptions::with_nonce_source(move || {
        let (high, low) = (generator.next_u64(), generator.next_u64());
        Nonce::new(&format!("{high:016X}{low:016X}")).unwrap()
    });
    let config = GenesisConfig::new(ShireId::new("racing").unwrap(), 1);
    let store = Store::init(vfs, root, config, options).unwrap();

    let trace: Arc<Mutex<Vec<String>>> = Arc::default();
    let torn: Arc<Mutex<Vec<String>>> = Arc::default();

    thread::scope(|scope| {
        for actor in 0..WRITERS {
            let (store, trace) = (store.clone(), Arc::clone(&trace));
            scope.spawn(move || {
                for step in 0..WRITES_EACH {
                    let (seen, commit) = store
                        .plan_and_apply(|view| {
                            let left = value(view.body(node("pair"), &doc("left")).unwrap());
                            let right = value(view.body(node("pair"), &doc("right")).unwrap());
                            let next = DocumentBody::new((left + 1).to_string().into_bytes());
                            let swaps = SwapSet::new([
                                Swap::new(node("pair"), doc("left"), next.clone()),
                                Swap::new(node("pair"), doc("right"), next),
                            ])
                            .unwrap();
                            ((left, right), Some(swaps))
                        })
                        .unwrap();
                    let commit = commit.unwrap();
                    trace.lock().unwrap().push(format!(
                        "writer {actor} step {step}: read {seen:?}, {:?} under {}",
                        commit.outcome(),
                        commit.nonce()
                    ));
                    assert!(commit.is_committed());
                }
            });
        }
        for actor in 0..READERS {
            let (store, trace, torn) = (store.clone(), Arc::clone(&trace), Arc::clone(&torn));
            scope.spawn(move || {
                for step in 0..SNAPSHOTS_EACH {
                    let outcome = store.snapshot(|view| {
                        (
                            value(view.body(node("pair"), &doc("left")).unwrap()),
                            value(view.body(node("pair"), &doc("right")).unwrap()),
                        )
                    });
                    let line = format!("reader {actor} step {step}: {outcome:?}");
                    if let Ok((left, right)) = outcome
                        && left != right
                    {
                        torn.lock().unwrap().push(line.clone());
                    }
                    trace.lock().unwrap().push(line);
                }
            });
        }
    });

    let torn = torn.lock().unwrap().clone();
    let final_value = value(
        store
            .read_document(node("pair"), &doc("left"))
            .ok()
            .and_then(|read| match read {
                canopy::Read::Found(body) => Some(body),
                _ => None,
            }),
    );
    if !torn.is_empty() || final_value != WRITERS * WRITES_EACH {
        for line in trace.lock().unwrap().iter() {
            println!("{line}");
        }
    }
    assert!(torn.is_empty(), "snapshots saw a pair torn apart: {torn:?}");
    assert_eq!(final_value, WRITERS * WRITES_EACH, "an increment was lost");
    assert!(
        store.commit_log_is_empty().unwrap(),
        "the last writer reset the log"
    );
}
