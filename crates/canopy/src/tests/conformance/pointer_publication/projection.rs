// Copyright (c) Michael Grier.

//! What the model and the store are compared on, read from a model state and from what the store
//! holds.

use std::collections::BTreeSet;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use acadia::{FileType, MemFs, Vfs, VfsErrorKind};
use serde_json::{Map, Value};

use super::super::corpus::Trace;
use crate::commit_log::{CommitLog, LogRecord};
use crate::store::{BLOB_TREE_DIR, NAMESPACE_TREE_DIR};
use crate::{
    BlobName, ContentDigest, DocumentBody, DocumentName, NodePath, Nonce, PathHash, Pointer,
};
/// The publication record's file name, which the store's layout fixes.
pub(super) const PUBLICATION: &str = ".publication";

/// What the model calls an absent pointer, record or prepared transaction.
const NONE: &str = "none";

/// What the model calls a transaction whose begin record is in the log and its prepare is not.
const BEGUN: &str = "begun";

/// A pointer, record or log the projection could not read.
const UNREADABLE: &str = "?";

/// The record's projection under a configuration without the record guard, whose model leaves the
/// record untouched while the code still writes one.
const UNCOMPARED: &str = "-";

/// The document the model is about, and how the trace's configuration lays it out.
pub(super) struct World {
    pub(super) root: PathBuf,
    pub(super) node: NodePath,
    pub(super) document: DocumentName,
    path_hash: PathHash,
    pub(super) pointers: usize,
    pub(super) blobs: usize,
    /// Each version the model names, with the digest of the body the bridge writes for it.
    versions: Vec<(String, ContentDigest)>,
    pub(super) initial: String,
    /// Whether the configuration has the record guard, so that the record is compared.
    guarded: bool,
}

impl World {
    pub(super) fn of(trace: &Trace) -> World {
        let count = |name: &str| {
            trace
                .constant(name)
                .parse::<usize>()
                .unwrap_or_else(|error| panic!("{}: {name}: {error}", trace.file))
        };
        let versions = trace
            .constant("Versions")
            .split('"')
            .skip(1)
            .step_by(2)
            .map(|version| (version.to_owned(), ContentDigest::of(version.as_bytes())))
            .collect();
        let node = NodePath::parse("N").expect("a valid node");
        let document = DocumentName::new("D").expect("a valid document name");
        World {
            root: PathBuf::from("store"),
            path_hash: PathHash::of(&node, &document),
            node,
            document,
            pointers: count("NPtr"),
            blobs: count("NBlob"),
            versions,
            initial: trace.constant("Initial").trim_matches('"').to_owned(),
            guarded: trace.constant("Fix") == "\"record\"",
        }
    }

    pub(super) fn body(&self, version: &str) -> DocumentBody {
        DocumentBody::new(version.as_bytes().to_vec())
    }

    fn named(&self, digest: ContentDigest) -> String {
        self.versions
            .iter()
            .find(|(_, known)| *known == digest)
            .map_or_else(|| UNREADABLE.to_owned(), |(name, _)| name.clone())
    }

    fn named_base32(&self, rendered: &str) -> String {
        self.versions
            .iter()
            .find(|(_, known)| known.to_base32() == rendered)
            .map_or_else(|| UNREADABLE.to_owned(), |(name, _)| name.clone())
    }

    /// The record as the projection compares it.
    fn compared(&self, record: String) -> String {
        if self.guarded {
            record
        } else {
            UNCOMPARED.to_owned()
        }
    }

    pub(super) fn entry(&self, index: usize) -> PathBuf {
        self.root
            .join(NAMESPACE_TREE_DIR)
            .join(format!("p{index}"))
            .join(self.node.relative_path())
            .join(self.document.as_str())
    }
}

/// What the model and the store are compared on: the document's pointer replicas, blob replicas
/// and publication record, each live and durable, and its transactions in the commit log.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Projection {
    ptr_live: Vec<String>,
    ptr_dur: Vec<String>,
    blob_live: Vec<BTreeSet<String>>,
    blob_dur: Vec<BTreeSet<String>>,
    record_live: String,
    record_dur: String,
    journalled: bool,
    tx_prep: String,
}

impl fmt::Debug for Projection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let blobs = |held: &[BTreeSet<String>]| {
            held.iter()
                .map(|set| set.iter().cloned().collect::<Vec<_>>().join(""))
                .collect::<Vec<_>>()
                .join("|")
        };
        write!(
            f,
            "ptr {}/{} blob {}/{} record {}/{} journalled {} txPrep {}",
            self.ptr_live.join(""),
            self.ptr_dur.join(""),
            blobs(&self.blob_live),
            blobs(&self.blob_dur),
            self.record_live,
            self.record_dur,
            self.journalled,
            self.tx_prep
        )
    }
}

fn text(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}

/// The projection of a model state.
pub(super) fn model(world: &World, state: &Map<String, Value>) -> Projection {
    let replicas = |name: &str| -> Vec<String> {
        state[name]
            .as_array()
            .unwrap_or_else(|| panic!("{name} is a sequence"))
            .iter()
            .map(text)
            .collect()
    };
    let holdings = |name: &str| -> Vec<BTreeSet<String>> {
        state[name]
            .as_array()
            .unwrap_or_else(|| panic!("{name} is a sequence"))
            .iter()
            .map(|set| {
                set.as_array()
                    .unwrap_or_else(|| panic!("{name} holds sets"))
                    .iter()
                    .map(text)
                    .collect()
            })
            .collect()
    };
    let record = |part: &str| {
        if world.guarded {
            text(&state["record"][part])
        } else {
            UNCOMPARED.to_owned()
        }
    };
    Projection {
        ptr_live: replicas("ptrLive"),
        ptr_dur: replicas("ptrDur"),
        blob_live: holdings("blobLive"),
        blob_dur: holdings("blobDur"),
        record_live: record("live"),
        record_dur: record("dur"),
        journalled: state["journalled"]
            .as_bool()
            .expect("journalled is a boolean"),
        tx_prep: text(&state["txPrep"]),
    }
}

/// The projection of what `mem` holds: live as it stands, durable as a crash now would leave it.
pub(super) fn observe(world: &World, mem: &MemFs) -> Projection {
    let durable = mem.fork();
    let durable = durable.simulate_restart();
    let (ptr_dur, blob_dur, record_dur) = view(world, &durable);
    let mut projection = Projection {
        ptr_live: Vec::new(),
        ptr_dur,
        blob_live: Vec::new(),
        blob_dur,
        record_live: String::new(),
        record_dur: world.compared(record_dur),
        journalled: false,
        tx_prep: String::new(),
    };
    read_live(world, mem, &mut projection);
    projection
}

/// As [`observe`], with what a crash would leave taken from `durable`, for a call that syncs nothing.
pub(super) fn observe_live(world: &World, mem: &MemFs, durable: &Projection) -> Projection {
    let mut projection = durable.clone();
    read_live(world, mem, &mut projection);
    projection
}

fn read_live(world: &World, mem: &MemFs, projection: &mut Projection) {
    let (ptr_live, blob_live, record_live) = view(world, mem);
    let (journalled, tx_prep) = journal(world, mem);
    projection.ptr_live = ptr_live;
    projection.blob_live = blob_live;
    projection.record_live = world.compared(record_live);
    projection.journalled = journalled;
    projection.tx_prep = tx_prep;
}

fn view(world: &World, fs: &dyn Vfs) -> (Vec<String>, Vec<BTreeSet<String>>, String) {
    let pointers = (0..world.pointers)
        .map(|index| match fs.read(&world.entry(index)) {
            Ok(bytes) => Pointer::parse(&bytes, world.path_hash).map_or_else(
                |_| UNREADABLE.to_owned(),
                |pointer| world.named(pointer.target().name().digest()),
            ),
            Err(error) if error.kind() == VfsErrorKind::NotFound => NONE.to_owned(),
            Err(_) => UNREADABLE.to_owned(),
        })
        .collect();

    let owner = world.path_hash.render();
    let blobs = (0..world.blobs)
        .map(|index| {
            let partition = world.root.join(BLOB_TREE_DIR).join(format!("p{index}"));
            let mut held = BTreeSet::new();
            for shard in fs.read_dir(&partition).unwrap_or_default() {
                if shard.file_type != FileType::Dir || shard.file_name.starts_with('.') {
                    continue;
                }
                for file in fs
                    .read_dir(&partition.join(&shard.file_name))
                    .unwrap_or_default()
                {
                    let name = &file.file_name;
                    if let Some((who, version)) = BlobName::owner_and_version(name)
                        && who == owner
                    {
                        held.insert(world.named_base32(version));
                    }
                }
            }
            held
        })
        .collect();

    let record = match fs.read(&world.root.join(PUBLICATION)) {
        Ok(bytes) => {
            let complete = match bytes.iter().rposition(|&byte| byte == b'\n') {
                Some(last) => &bytes[..=last],
                None => &bytes[..0],
            };
            let named: Vec<String> = complete
                .split(|&byte| byte == b'\n')
                .filter(|line| !line.is_empty())
                .map(|line| {
                    serde_json::from_slice::<Value>(line)
                        .ok()
                        .and_then(|record| {
                            record["digest"].as_str().and_then(ContentDigest::from_hex)
                        })
                        .map_or_else(|| UNREADABLE.to_owned(), |digest| world.named(digest))
                })
                .collect();
            if named.is_empty() {
                NONE.to_owned()
            } else {
                named.join("+")
            }
        }
        Err(error) if error.kind() == VfsErrorKind::NotFound => NONE.to_owned(),
        Err(_) => UNREADABLE.to_owned(),
    };
    (pointers, blobs, record)
}

/// Whether the log holds an ended transaction of the document, and the version a prepared one
/// that has not ended names, or `begun` for one whose begin record names the document and that
/// has no prepare record.
fn journal(world: &World, mem: &MemFs) -> (bool, String) {
    let log = CommitLog::new(Arc::new(mem.another_process()), &world.root);
    let Ok(records) = log.read() else {
        return (false, UNREADABLE.to_owned());
    };
    let mut prepared: Vec<(Nonce, String)> = Vec::new();
    let mut ended: Vec<Nonce> = Vec::new();
    let mut begun: Vec<Nonce> = Vec::new();
    for record in &records {
        match record {
            LogRecord::Begin { txn, targets } => {
                if targets
                    .iter()
                    .any(|(node, document)| node == &world.node && document == &world.document)
                {
                    begun.push(txn.clone());
                }
            }
            LogRecord::Prepare { txn, prepared: set } => {
                for swap in set.swaps() {
                    if swap.node() == &world.node && swap.document() == &world.document {
                        let version = swap
                            .digest()
                            .map_or_else(|| NONE.to_owned(), |digest| world.named(digest));
                        prepared.push((txn.clone(), version));
                    }
                }
            }
            LogRecord::End { txn } => ended.push(txn.clone()),
        }
    }
    let journalled = prepared.iter().any(|(txn, _)| ended.contains(txn));
    let unprepared = begun
        .iter()
        .any(|txn| !ended.contains(txn) && !prepared.iter().any(|(id, _)| id == txn));
    let tx_prep = match prepared.iter().find(|(txn, _)| !ended.contains(txn)) {
        Some((_, version)) => version.clone(),
        None if unprepared => BEGUN.to_owned(),
        None => NONE.to_owned(),
    };
    (journalled, tx_prep)
}
