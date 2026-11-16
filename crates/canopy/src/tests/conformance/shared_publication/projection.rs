// Copyright (c) Michael Grier.

use std::collections::BTreeSet;
use std::path::PathBuf;
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
    track_directories: bool,
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
            track_directories: true,
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
pub(super) struct Projection {
    pub(super) live: Documents,
    pub(super) durable: Documents,
    pub(super) log: String,
    pub(super) prepared: Option<(usize, usize)>,
    pub(super) directories: Option<(usize, usize)>,
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
        prepared: state["prep"]["ver"].as_u64().map(|version| {
            (
                usize::try_from(
                    state["prep"]["doc"]
                        .as_u64()
                        .expect("prepared document is an index"),
                )
                .expect("index fits"),
                usize::try_from(version).expect("version fits"),
            )
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
                if shard.file_type != FileType::Dir
                    || shard.file_name.to_string_lossy().starts_with('.')
                {
                    continue;
                }
                for file in fs
                    .read_dir(&partition.join(&shard.file_name))
                    .map_err(|error| error.to_string())?
                {
                    if let Some((owner, version)) =
                        BlobName::owner_and_version(&file.file_name.to_string_lossy())
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
    durable.simulate_crash();
    let records = CommitLog::new(Arc::new(mem.another_process()), &world.root)
        .read()
        .map_err(|error| error.to_string())?;
    let (log, prepared) = match records.last() {
        None => ("empty", None),
        Some(LogRecord::Begin { .. }) => ("begun", None),
        Some(LogRecord::End { .. }) => ("ended", None),
        Some(LogRecord::Prepare { prepared, .. }) => {
            let swaps = prepared.swaps();
            if swaps.len() != 1 {
                return Err("the model prepares one document".to_owned());
            }
            let swap = &swaps[0];
            let document = world
                .documents
                .iter()
                .position(|document| {
                    swap.node() == &document.node && swap.document() == &document.name
                })
                .ok_or("prepare names an unknown document")?;
            (
                "prepared",
                Some((
                    document + 1,
                    world.version(swap.digest().ok_or("prepare removes a document")?)?,
                )),
            )
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
        directories,
    })
}
