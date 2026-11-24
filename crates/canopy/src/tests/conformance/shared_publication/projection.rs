// Copyright (c) Michael Grier.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use acadia::{FileType, MemFs, Vfs, VfsErrorKind};
use serde_json::{Map, Value};

use super::super::corpus::Trace;
use crate::commit_log::{CommitLog, LogRecord};
use crate::store::{
    BLOB_TREE_DIR, DIR_SYNC_DEBT_NAME, NAMESPACE_TREE_DIR, PUBLICATION_RECORD_NAME,
};
use crate::{
    BlobName, ContentDigest, DocumentBody, DocumentName, NodePath, Nonce, PathHash, Pointer,
    Precondition,
};

pub(super) struct Document {
    pub(super) node: NodePath,
    pub(super) name: DocumentName,
    hash: PathHash,
}

pub(super) struct World {
    pub(super) root: PathBuf,
    pub(super) documents: Vec<Document>,
    pub(super) pointers: usize,
    pub(super) blobs: usize,
    versions: Vec<ContentDigest>,
    guarded: bool,
    nonces: AtomicU64,
    pub(super) initial_directory_debt: usize,
    pub(super) initial_node_removed: bool,
    track_directories: bool,
    track_nodes: bool,
}

impl World {
    pub(super) fn new(
        documents: usize,
        pointers: usize,
        blobs: usize,
        maximum: usize,
        guarded: bool,
    ) -> World {
        World {
            root: super::super::coordination::projection::root().to_path_buf(),
            documents: (1..=documents)
                .map(|index| {
                    let node = NodePath::parse("N").expect("the model node is valid");
                    let name = DocumentName::new(&format!("D{index}"))
                        .expect("the model document is valid");
                    let hash = PathHash::of(&node, &name);
                    Document { node, name, hash }
                })
                .collect(),
            pointers,
            blobs,
            versions: (0..=maximum)
                .map(|version| ContentDigest::of(Self::body(version).as_bytes()))
                .collect(),
            guarded,
            nonces: AtomicU64::new(0),
            initial_directory_debt: 0,
            initial_node_removed: false,
            track_directories: true,
            track_nodes: false,
        }
    }

    pub(super) fn of(trace: &Trace) -> World {
        let initial = &trace.states[0];
        let documents = initial["ptrLive"]
            .as_array()
            .expect("document pointers are a sequence")
            .len();
        let pointers = initial["ptrLive"][0]
            .as_array()
            .expect("pointer replicas are a sequence")
            .len();
        let blobs = initial["blobLive"][0]
            .as_array()
            .expect("blob replicas are a sequence")
            .len();
        let maximum = trace
            .states
            .iter()
            .map(|state| state["gen"].as_u64().expect("the generation is natural"))
            .max()
            .unwrap_or_default();
        let mut world = Self::new(
            documents,
            pointers,
            blobs,
            usize::try_from(maximum).expect("the generation fits"),
            trace.constant("Fix") == "\"record\"",
        );
        world.track_directories = initial.contains_key("directoryLive");
        world.track_nodes = initial.contains_key("nodeLive");
        world.initial_node_removed = trace
            .constants
            .get("InitialNodeRemoved")
            .is_some_and(|value| value == "TRUE");
        world.initial_directory_debt = initial
            .get("directoryDurable")
            .and_then(Value::as_u64)
            .map(|count| usize::try_from(count).expect("directory count fits"))
            .unwrap_or_default();
        world
    }

    pub(super) fn body(version: usize) -> DocumentBody {
        DocumentBody::new(format!("version-{version}").into_bytes())
    }

    pub(super) fn nonce(&self, actor: usize) -> Nonce {
        let serial = self.nonces.fetch_add(1, Ordering::AcqRel) + 1;
        Nonce::new(&format!("A{actor}N{serial}")).expect("valid replay nonce")
    }

    pub(super) fn directory_record(&self) -> PathBuf {
        self.root.join(DIR_SYNC_DEBT_NAME)
    }

    pub(super) fn directory_target(&self) -> PathBuf {
        self.entry(0, 0)
            .parent()
            .expect("a document has a parent")
            .to_path_buf()
    }

    pub(super) fn model_node(&self) -> NodePath {
        self.documents[0].node.clone()
    }

    pub(super) fn model_node_at_document(&self) -> NodePath {
        let document = &self.documents[0];
        NodePath::parse(&format!("{}/{}", document.node, document.name.as_str()))
            .expect("a document entry is a valid node path")
    }

    pub(super) fn model_node_below_document(&self) -> NodePath {
        let document = &self.documents[0];
        NodePath::parse(&format!(
            "{}/{}/child",
            document.node,
            document.name.as_str()
        ))
        .expect("a path below a document is a valid node path")
    }

    pub(super) fn seed_directory_debt(&self, mem: &MemFs) {
        if self.initial_directory_debt == 0 {
            return;
        }
        let target = self.directory_target();
        let relative = target
            .strip_prefix(&self.root)
            .expect("the directory is beneath the store");
        let record = format!("{}\n", relative.display()).repeat(self.initial_directory_debt);
        mem.write(&self.directory_record(), record.as_bytes())
            .expect("seed the directory record");
        mem.fsync(&self.directory_record())
            .expect("make pending debt durable");
        mem.fsync(&self.root).expect("make the record name durable");
        mem.truncate(&self.directory_record(), 0)
            .expect("leave only the durable debt");
    }

    pub(super) fn entry(&self, document: usize, replica: usize) -> PathBuf {
        let document = &self.documents[document];
        self.root
            .join(NAMESPACE_TREE_DIR)
            .join(format!("p{replica}"))
            .join(document.node.relative_path())
            .join(document.name.as_str())
    }

    pub(super) fn node_path(&self, replica: usize) -> PathBuf {
        self.root
            .join(NAMESPACE_TREE_DIR)
            .join(format!("p{replica}"))
            .join(self.documents[0].node.relative_path())
    }

    pub(super) fn node_replica(&self, path: &Path) -> Option<usize> {
        (0..self.pointers).find(|replica| self.node_path(*replica) == path)
    }

    pub(super) fn node_parent(&self, path: &Path) -> Option<usize> {
        (0..self.pointers).find(|replica| self.node_path(*replica).parent() == Some(path))
    }

    pub(super) fn node_exists(&self, fs: &dyn Vfs, replica: usize) -> Result<bool, String> {
        let path = self.node_path(replica);
        match fs.metadata(&path) {
            Ok(metadata) => Ok(metadata.is_dir()),
            Err(error)
                if matches!(
                    error.kind(),
                    VfsErrorKind::NotFound | VfsErrorKind::NotADirectory
                ) =>
            {
                Ok(false)
            }
            Err(error) => Err(error.to_string()),
        }
    }

    fn version(&self, digest: ContentDigest) -> Result<usize, String> {
        self.versions
            .iter()
            .position(|known| *known == digest)
            .ok_or_else(|| format!("unknown document digest {}", digest.to_hex()))
    }

    fn rendered_version(&self, digest: &str) -> Result<usize, String> {
        self.versions
            .iter()
            .position(|known| known.to_base32() == digest)
            .ok_or_else(|| format!("unknown blob digest {digest}"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Documents {
    pub(super) pointers: Vec<Vec<Option<usize>>>,
    pub(super) blobs: Vec<Vec<BTreeSet<usize>>>,
    pub(super) records: Vec<Option<usize>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PreparedPrecondition {
    Unconditional,
    ExpectedDigest(usize),
    ExpectedAbsent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PreparedSwap {
    pub(super) document: usize,
    pub(super) version: Option<usize>,
    pub(super) precondition: PreparedPrecondition,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PreparedNodeRemoval {
    Node,
    AtDocument,
    BelowDocument,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProjectedNodes {
    live: Vec<Vec<bool>>,
    durable: Vec<Vec<bool>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Projection {
    pub(super) live: Documents,
    pub(super) durable: Documents,
    pub(super) log: String,
    pub(super) prepared: Vec<PreparedSwap>,
    pub(super) node_removals: Vec<PreparedNodeRemoval>,
    nodes: Option<ProjectedNodes>,
    pub(super) directories: Option<(usize, usize)>,
}

fn model_prepared_swap(swap: &Value) -> PreparedSwap {
    let precondition = match swap.get("precondition").and_then(Value::as_str) {
        Some("digest") => PreparedPrecondition::ExpectedDigest(
            usize::try_from(
                swap["expected"]
                    .as_u64()
                    .expect("digest expectation is natural"),
            )
            .expect("version fits"),
        ),
        Some("absent") => PreparedPrecondition::ExpectedAbsent,
        Some("unconditional") | None => PreparedPrecondition::Unconditional,
        _ => panic!("prepared swap has an unknown precondition"),
    };
    let version = if swap.get("effect").and_then(Value::as_str) == Some("remove") {
        None
    } else {
        Some(
            usize::try_from(swap["ver"].as_u64().expect("prepared version is natural"))
                .expect("version fits"),
        )
    };
    PreparedSwap {
        document: usize::try_from(swap["doc"].as_u64().expect("prepared document is an index"))
            .expect("index fits"),
        version,
        precondition,
    }
}

pub(super) fn model(world: &World, state: &Map<String, Value>) -> Projection {
    let documents = |pointer_field: &str, blob_field: &str, record_part: &str| Documents {
        pointers: state[pointer_field]
            .as_array()
            .expect("documents are a sequence")
            .iter()
            .map(|replicas| {
                replicas
                    .as_array()
                    .expect("pointers are a sequence")
                    .iter()
                    .map(|value| {
                        value
                            .as_u64()
                            .map(|version| usize::try_from(version).expect("version fits"))
                    })
                    .collect()
            })
            .collect(),
        blobs: state[blob_field]
            .as_array()
            .expect("documents are a sequence")
            .iter()
            .map(|replicas| {
                replicas
                    .as_array()
                    .expect("blob replicas are a sequence")
                    .iter()
                    .map(|versions| {
                        versions
                            .as_array()
                            .expect("blob versions are a set")
                            .iter()
                            .map(|value| {
                                usize::try_from(value.as_u64().expect("version is natural"))
                                    .expect("version fits")
                            })
                            .collect()
                    })
                    .collect()
            })
            .collect(),
        records: state["record"]
            .as_array()
            .expect("records are a sequence")
            .iter()
            .map(|record| {
                world
                    .guarded
                    .then(|| record[record_part].as_u64())
                    .flatten()
                    .map(|version| usize::try_from(version).expect("version fits"))
            })
            .collect(),
    };
    Projection {
        live: documents("ptrLive", "blobLive", "live"),
        durable: documents("ptrDur", "blobDur", "dur"),
        log: state["log"].as_str().expect("log is a phase").to_owned(),
        prepared: if state["log"] == "prepared" {
            match state["prep"].as_array() {
                Some(swaps) => swaps.iter().map(model_prepared_swap).collect(),
                None => vec![model_prepared_swap(&state["prep"])],
            }
        } else {
            Vec::new()
        },
        node_removals: if state["log"] == "prepared" {
            state
                .get("removeNodes")
                .and_then(Value::as_array)
                .map(|paths| {
                    paths
                        .iter()
                        .map(
                            |path| match path.as_array().expect("node path is a sequence").len() {
                                1 => PreparedNodeRemoval::Node,
                                2 => PreparedNodeRemoval::AtDocument,
                                3 => PreparedNodeRemoval::BelowDocument,
                                _ => panic!("unknown model node path"),
                            },
                        )
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        },
        nodes: state.get("nodeLive").map(|_| {
            let nodes = |field: &str| {
                state[field]
                    .as_array()
                    .expect("node directories are a sequence")
                    .iter()
                    .map(|replicas| {
                        replicas
                            .as_array()
                            .expect("node replicas are a sequence")
                            .iter()
                            .map(|value| value.as_bool().expect("node existence is boolean"))
                            .collect()
                    })
                    .collect()
            };
            ProjectedNodes {
                live: nodes("nodeLive"),
                durable: nodes("nodeDur"),
            }
        }),
        directories: state
            .get("directoryLive")
            .and_then(Value::as_u64)
            .map(|live| {
                (
                    usize::try_from(live).expect("directory count fits"),
                    usize::try_from(
                        state["directoryDurable"]
                            .as_u64()
                            .expect("durable directory count"),
                    )
                    .expect("directory count fits"),
                )
            }),
    }
}

fn read_documents(world: &World, fs: &dyn Vfs) -> Result<Documents, String> {
    let mut pointers = Vec::new();
    let mut blobs = Vec::new();
    for (document_index, document) in world.documents.iter().enumerate() {
        let mut replicas = Vec::new();
        for replica in 0..world.pointers {
            replicas.push(match fs.read(&world.entry(document_index, replica)) {
                Ok(bytes) => Some(
                    world.version(
                        Pointer::parse(&bytes, document.hash)
                            .map_err(|error| error.to_string())?
                            .target()
                            .name()
                            .digest(),
                    )?,
                ),
                Err(error) if error.kind() == VfsErrorKind::NotFound => None,
                Err(error) => return Err(error.to_string()),
            });
        }
        pointers.push(replicas);
        let mut replicas = Vec::new();
        for replica in 0..world.blobs {
            let partition = world.root.join(BLOB_TREE_DIR).join(format!("p{replica}"));
            let mut versions = BTreeSet::new();
            for shard in fs.read_dir(&partition).map_err(|error| error.to_string())? {
                if shard.file_type != FileType::Dir || shard.file_name.starts_with('.') {
                    continue;
                }
                for file in fs
                    .read_dir(&partition.join(&shard.file_name))
                    .map_err(|error| error.to_string())?
                {
                    if let Some((owner, version)) = BlobName::owner_and_version(&file.file_name)
                        && owner == document.hash.render()
                    {
                        versions.insert(world.rendered_version(version)?);
                    }
                }
            }
            replicas.push(versions);
        }
        blobs.push(replicas);
    }
    let mut records = vec![None; world.documents.len()];
    if world.guarded {
        match fs.read(&world.root.join(PUBLICATION_RECORD_NAME)) {
            Ok(bytes) => {
                let complete = bytes
                    .iter()
                    .rposition(|byte| *byte == b'\n')
                    .map_or(&bytes[..0], |last| &bytes[..=last]);
                for line in complete
                    .split(|byte| *byte == b'\n')
                    .filter(|line| !line.is_empty())
                {
                    let record: Value =
                        serde_json::from_slice(line).map_err(|error| error.to_string())?;
                    let document = world
                        .documents
                        .iter()
                        .position(|document| {
                            record["node"] == document.node.to_string()
                                && record["document"] == document.name.as_str()
                        })
                        .ok_or("publication record names an unknown document")?;
                    let digest = record["digest"]
                        .as_str()
                        .and_then(ContentDigest::from_hex)
                        .ok_or("publication record has an invalid digest")?;
                    if records[document].replace(world.version(digest)?).is_some() {
                        return Err("duplicate publication records".to_owned());
                    }
                }
            }
            Err(error) if error.kind() == VfsErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(Documents {
        pointers,
        blobs,
        records,
    })
}

pub(super) fn observe(world: &World, mem: &MemFs) -> Result<Projection, String> {
    let durable = mem.fork();
    let durable = durable.simulate_restart();
    let records = CommitLog::new(Arc::new(mem.another_process()), &world.root)
        .read()
        .map_err(|error| error.to_string())?;
    let (log, prepared, node_removals) = match records.last() {
        None => ("empty", Vec::new(), Vec::new()),
        Some(LogRecord::Begin { .. }) => ("begun", Vec::new(), Vec::new()),
        Some(LogRecord::End { .. }) => ("ended", Vec::new(), Vec::new()),
        Some(LogRecord::Prepare { prepared, .. }) => {
            let mut swaps = Vec::with_capacity(prepared.swaps().len());
            for swap in prepared.swaps() {
                let document = world
                    .documents
                    .iter()
                    .position(|document| {
                        swap.node() == &document.node && swap.document() == &document.name
                    })
                    .ok_or("prepare names an unknown document")?;
                let precondition = match swap.precondition() {
                    Precondition::Unconditional => PreparedPrecondition::Unconditional,
                    Precondition::ExpectedDigest(digest) => {
                        PreparedPrecondition::ExpectedDigest(world.version(digest)?)
                    }
                    Precondition::ExpectedAbsent => PreparedPrecondition::ExpectedAbsent,
                };
                swaps.push(PreparedSwap {
                    document: document + 1,
                    version: swap
                        .digest()
                        .map(|digest| world.version(digest))
                        .transpose()?,
                    precondition,
                });
            }
            let node_removals = prepared
                .node_removals()
                .iter()
                .map(|path| {
                    if path == &world.model_node() {
                        PreparedNodeRemoval::Node
                    } else if path == &world.model_node_at_document() {
                        PreparedNodeRemoval::AtDocument
                    } else if path == &world.model_node_below_document() {
                        PreparedNodeRemoval::BelowDocument
                    } else {
                        panic!("prepare names an unknown node removal")
                    }
                })
                .collect();
            ("prepared", swaps, node_removals)
        }
    };
    let directory_count = |fs: &dyn Vfs| -> Result<usize, String> {
        match fs.read(&world.directory_record()) {
            Ok(bytes) => Ok(bytes
                .split_inclusive(|byte| *byte == b'\n')
                .filter(|line| line.len() > 1 && line.last() == Some(&b'\n'))
                .count()),
            Err(error) if error.kind() == VfsErrorKind::NotFound => Ok(0),
            Err(error) => Err(error.to_string()),
        }
    };
    let directories = if world.track_directories {
        Some((directory_count(mem)?, directory_count(&durable)?))
    } else {
        None
    };
    Ok(Projection {
        live: read_documents(world, mem)?,
        durable: read_documents(world, &durable)?,
        log: log.to_owned(),
        prepared,
        node_removals,
        nodes: world
            .track_nodes
            .then(|| {
                let live = (0..world.documents.len())
                    .map(|_| {
                        (0..world.pointers)
                            .map(|replica| world.node_exists(mem, replica))
                            .collect()
                    })
                    .collect::<Result<_, String>>()?;
                let durable = (0..world.documents.len())
                    .map(|_| {
                        (0..world.pointers)
                            .map(|replica| world.node_exists(&durable, replica))
                            .collect()
                    })
                    .collect::<Result<_, String>>()?;
                Ok::<_, String>(ProjectedNodes { live, durable })
            })
            .transpose()?,
        directories,
    })
}
