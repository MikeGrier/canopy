// Copyright (c) Michael Grier.

//! Unit tests for the canopy value types and store: path validation, content-digest
//! determinism over opaque bodies, swap-set construction, the layout, and the
//! transactional apply. Disk-free and deterministic over `acadia`'s `MemFs`.

use crate::{
    ContentDigest, DocumentBody, Effect, NodePath, NodePathError, Precondition, Swap, SwapSet,
    SwapSetError,
};

mod apply;
mod attachment;
mod blob;
mod commit_log;
mod conformance;
mod creation;
mod digest;
mod hierarchy;
mod layout;
mod lock;
mod management;
mod migration;
mod modification;
mod names;
mod namespace;
mod naming;
mod node_path;
mod node_removal;
mod nonce_source;
mod pointer;
mod publication;
mod quiescence;
mod recovery;
mod recovery_run;
mod removal;
// Under `--cfg canopy_loom` the doorbell's atomics are loom's, which work only inside a loom model.
#[cfg(not(canopy_loom))]
mod ring;
#[cfg(canopy_loom)]
mod ring_loom;
mod session;
mod shared_count;
mod store_nonce;
mod surface;
mod transaction;
mod verbs;
mod write_surface;

/// A node path for tests, panicking on an invalid one.
///
/// The store is keyed on paths since `D222`, and a one-component path is what the flat
/// namespace's names became.
fn name(raw: &str) -> NodePath {
    NodePath::parse(raw).expect("valid test path")
}

fn body(bytes: &[u8]) -> DocumentBody {
    DocumentBody::new(bytes.to_vec())
}

fn transform_document(
    store: &Store,
    node: impl Into<NodePath>,
    document: &DocumentName,
    mut transform: impl FnMut(Option<&DocumentBody>) -> DocumentBody,
) -> Result<crate::WriteOutcome<(DocumentBody, crate::Commit)>, StoreError> {
    let node = node.into();
    let document = document.clone();
    store.transform(move |transaction| {
        let current = transaction.read_document(node.clone(), &document)?;
        let output = transform(current.as_ref());
        transaction.write(node.clone(), document.clone(), output.clone());
        Ok(output)
    })
}

/// Reading a document's body in a test, where no writer is competing.
///
/// [`read_once`](crate::read_once) is the right driver here precisely because a test store
/// has one thread: being superseded would be an anomaly worth failing on, not contention to
/// wait out. The trait exists so a test can keep saying `store.read_body(..)` rather than
/// restructuring every assertion around a free function.
trait ReadBody {
    fn read_body(
        &self,
        node: impl Into<NodePath>,
        document: &DocumentName,
    ) -> Result<Option<DocumentBody>, StoreError>;
}

impl ReadBody for Store {
    fn read_body(
        &self,
        node: impl Into<NodePath>,
        document: &DocumentName,
    ) -> Result<Option<DocumentBody>, StoreError> {
        let node = node.into();
        crate::read_once(self, &node, document)
    }
}

use std::path::PathBuf;
use std::sync::Arc;

use crate::store::NAMESPACE_TREE_DIR;
use crate::{GenesisConfig, MARKER_NAME, Store, StoreError, StoreId};

/// The root of namespace replica `index`.
fn namespace_root(store: &Store, index: usize) -> PathBuf {
    store
        .root()
        .join(NAMESPACE_TREE_DIR)
        .join(format!("p{index}"))
}
use acadia::{MemFs, Vfs};

pub(crate) const MEM_ROOT: &str = if cfg!(windows) { "C:/" } else { "/" };

/// A fresh in-memory filesystem with an existing (empty) store root directory.
///
/// Store tests run disk-free and deterministically against the `acadia`
/// `MemFs` backend (D55).
fn fresh_root() -> (Arc<dyn Vfs>, PathBuf) {
    let vfs: Arc<dyn Vfs> = Arc::new(MemFs::new());
    let root = PathBuf::from(MEM_ROOT).join("store");
    vfs.create_dir_all(&root).expect("create root");
    (vfs, root)
}

fn test_config() -> GenesisConfig {
    GenesisConfig::new(StoreId::new("test-store").expect("non-empty id"), 12_345)
}

pub(crate) fn test_agent() -> crate::Agent {
    crate::Agent::new("canopy-tests", "test", "Canopy test fixture", 1)
}

fn marker_with_agent_metadata(bytes: &[u8]) -> Vec<u8> {
    let agent = test_agent();
    let mut marker: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    marker["metadata"] = serde_json::json!({
        "agent_identifier": agent.identifier(),
        "agent_version": agent.version(),
        "agent_description": agent.description(),
        "canopy_schema": {
            "current": crate::CANOPY_SCHEMA_VERSION,
            "oldest_reader": crate::CANOPY_SCHEMA_VERSION,
        },
        "agent_schema": { "current": agent.schema_version(), "oldest_reader": agent.schema_version() },
    });
    serde_json::to_vec(&marker).unwrap()
}

/// Records the default coordination implementation on a root whose marker a test wrote by hand,
/// as creating the root would have, so that opening it is not refused as a root no creation made.
fn reserve_by_hand(vfs: &Arc<dyn Vfs>, root: &std::path::Path) {
    use acadia::coord::file::FileFactory;
    use acadia::coord::{CoordinationFactory, CreationNonce};
    FileFactory::new()
        .open(Arc::clone(vfs), root)
        .expect("a participant")
        .reserve(b"made by hand", CreationNonce::from_bytes([0; 16]))
        .expect("record the implementation");
}

use crate::{DocumentName, DocumentNameError};

fn doc(raw: &str) -> DocumentName {
    DocumentName::new(raw).expect("valid test document name")
}

/// A freshly initialized store on an in-memory filesystem, plus a handle to that
/// same filesystem for tests that inspect the raw on-disk layout.
fn fresh_store() -> (Arc<dyn Vfs>, Store) {
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs.clone(), root, test_config()).expect("init store");
    (vfs, store)
}

/// Drops `store` and opens its root again, resolving what it left -- the restart of a writer
/// these recovery tests mean.
///
/// Dropping the original attachment ensures recovery cannot rely on its cached durability state.
fn reopen(vfs: &Arc<dyn Vfs>, store: Store) -> Result<Store, StoreError> {
    let root = store.root().to_path_buf();
    drop(store);
    open_resolved(Arc::clone(vfs), root)
}

/// Drops `store`, opens its root again and recovers through the settling of every transaction,
/// stopping before collection, while the log still shows how each was closed: resolving and a
/// finished recovery leave it empty.
fn reopen_settled(vfs: &Arc<dyn Vfs>, store: Store) -> Store {
    let root = store.root().to_path_buf();
    drop(store);
    let reopened = Store::open_unsourced(Arc::clone(vfs), root).unwrap();
    let mut run = reopened.recovery().unwrap();
    while run.progress().phase() != crate::RecoveryPhase::Collection {
        run.step()
            .unwrap()
            .agent_stamp
            .expect("agent version stamp");
    }
    reopened
}

/// Opens `root` and resolves what writers left, as a writer that means to write does.
fn open_resolved(vfs: Arc<dyn Vfs>, root: impl Into<PathBuf>) -> Result<Store, StoreError> {
    let store = Store::open_unsourced(vfs, root)?;
    store.resolve()?.agent_stamp.expect("agent version stamp");
    Ok(store)
}

use crate::{Nonce, NonceError};

fn nonce(raw: &str) -> Nonce {
    Nonce::new(raw).expect("valid nonce")
}

/// The file names in namespace replica `index`'s directory for the node `record`, for
/// asserting no `.tmp.<nonce>` staging orphan survives a commit.
///
/// One directory per path component since `D222`, so a one-component path is one
/// directory below the replica root -- there is no synthesized shard tier above it.
fn node_entries(vfs: &Arc<dyn Vfs>, store: &Store, index: usize) -> Vec<String> {
    let node_dir = namespace_root(store, index).join("record");
    let mut names: Vec<String> = vfs
        .read_dir(&node_dir)
        .expect("read node dir")
        .into_iter()
        .map(|entry| entry.file_name)
        .collect();
    names.sort();
    names
}

use crate::StoreOptions;
use std::thread;

/// A deterministic nonce source: `N1`, `N2`, `N3`, ... .
fn counting_nonce_source() -> impl FnMut() -> Nonce + Send + 'static {
    let mut n: u64 = 0;
    move || {
        n += 1;
        Nonce::new(&format!("N{n}")).unwrap()
    }
}

/// A fresh store drawing from [`counting_nonce_source`], for tests of the public write verbs.
fn sourced_store() -> (Arc<dyn Vfs>, Store) {
    let (vfs, root) = fresh_root();
    let options = StoreOptions::with_nonce_source(test_agent(), counting_nonce_source());
    let store = Store::init(vfs.clone(), root, test_config(), options).expect("init store");
    (vfs, store)
}

fn seeded_store() -> (Arc<dyn Vfs>, Store) {
    sourced_store()
}

fn text(store: &Store, node: &str, document: &str) -> Option<String> {
    store
        .read_body(name(node), &doc(document))
        .unwrap()
        .map(|body| String::from_utf8(body.as_bytes().to_vec()).unwrap())
}

use crate::commit_log::{
    CommitLog, CommitLogError, LOG_NAME, LogRecord, PreparedSet, PreparedSwap,
};

/// Matches a staging path in the **namespace** tree, excluding the blob tree.
///
/// Both trees use `p{i}` replica directories and a [`STAGING_DIR`](crate::store::STAGING_DIR)
/// beneath them, so a matcher naming either component alone cannot say which tree it means.
/// A fault meant for the pointer move -- which happens after the commit point -- would
/// otherwise fire on the blob ingest in phase 2, landing the test on a completely different
/// code path while still looking like it faulted what it named.
fn namespace_tree_staging() -> acadia::PathMatcher {
    use crate::store::{NAMESPACE_TREE_DIR, STAGING_DIR};
    acadia::PathMatcher::predicate(|path| {
        path.components().any(|c| c.as_os_str() == STAGING_DIR)
            && path
                .components()
                .any(|c| c.as_os_str() == NAMESPACE_TREE_DIR)
    })
}

/// A `begin` record naming no ingest targets (`D329`).
///
/// The tests using this hand-craft a log to model an interrupted transaction and then drive
/// recovery over it. Naming no targets says "this transaction ingested nothing to collect",
/// which keeps the reconstruction's collection phase out of what the test is asserting.
fn begin(txn: &Nonce) -> LogRecord {
    LogRecord::Begin {
        txn: txn.clone(),
        targets: Vec::new(),
    }
}

/// The `prepare` payload a transaction over `swaps` would have hardened (`CVS-14`).
///
/// Tests that hand-craft a log to model an interrupted transaction need the record the
/// apply path would have written. That path takes each digest from the blob the content
/// store actually wrote; here there is no such write, so the digest is taken from the
/// body -- which is the same value, since both are SHA-256 over the same bytes.
///
/// **Test-only, deliberately.** Offering this on the production path would let a caller
/// record a digest derived from the body it *meant* to store rather than from the bytes
/// that were stored, which is exactly the coupling `D327` removes.
fn prepared(swaps: &SwapSet) -> PreparedSet {
    let entries = swaps
        .swaps()
        .iter()
        .map(|swap| match swap.body() {
            Some(body) => PreparedSwap::writing(swap, body.digest()),
            None => PreparedSwap::removing(swap),
        })
        .collect();
    PreparedSet::new(entries, swaps.node_removals().to_vec())
}

/// A commit log over a fresh in-memory store root, plus its backing vfs (for tests
/// that inject raw bytes to model a torn or corrupt tail).
fn fresh_log() -> (Arc<dyn Vfs>, CommitLog) {
    let (vfs, root) = fresh_root();
    let log = CommitLog::new(Arc::clone(&vfs), &root);
    (vfs, log)
}

/// A begin/prepare/end triple over one two-swap transaction, for round-trip tests.
fn sample_swaps() -> SwapSet {
    SwapSet::new([
        Swap::new(name("alpha"), doc("record"), body(b"first")),
        Swap::conditional(
            name("bravo"),
            doc("record"),
            body(b"second"),
            body(b"prior").digest(),
        ),
    ])
    .expect("distinct targets")
}

use crate::{RejectedPrecondition, TransactionOutcome};

/// Whether namespace replica `index` still holds a `.staging/<txn>` tree for `txn`.
fn staging_exists(vfs: &Arc<dyn Vfs>, store: &Store, index: usize, txn: &Nonce) -> bool {
    let staging = namespace_root(store, index)
        .join(".staging")
        .join(txn.path_component().as_ref());
    vfs.exists(&staging).unwrap()
}
