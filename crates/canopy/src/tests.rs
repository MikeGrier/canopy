// Copyright (c) Michael Grier.

//! Unit tests for the canopy value types and store: path validation, content-digest
//! determinism over opaque bodies, swap-set construction, the layout, and the
//! transactional apply. Disk-free and deterministic over `acadia`'s `MemFs`.

use crate::{
    ContentDigest, DocumentBody, Effect, NodePath, NodePathError, Precondition, Swap, SwapSet,
    SwapSetError,
};

mod apply;
mod blob;
mod commit_log;
mod digest;
mod hierarchy;
mod layout;
mod lock;
mod modification;
mod names;
mod namespace;
mod naming;
mod node_path;
mod node_removal;
mod nonce_source;
mod pointer;
mod quiescence;
mod recovery;
mod removal;
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
use crate::{GenesisConfig, MARKER_NAME, ShireId, Store, StoreError};

/// The root of namespace replica `index`.
fn namespace_root(store: &Store, index: usize) -> PathBuf {
    store
        .root()
        .join(NAMESPACE_TREE_DIR)
        .join(format!("p{index}"))
}
use acadia::{MemFs, Vfs};

/// A fresh in-memory filesystem with an existing (empty) Shire root directory.
///
/// Store tests run disk-free and deterministically against the `acadia`
/// `MemFs` backend (D55).
fn fresh_root() -> (Arc<dyn Vfs>, PathBuf) {
    let vfs: Arc<dyn Vfs> = Arc::new(MemFs::new());
    let root = PathBuf::from("shire");
    vfs.create_dir_all(&root).expect("create root");
    (vfs, root)
}

fn test_config() -> GenesisConfig {
    GenesisConfig::new(ShireId::new("test-shire").expect("non-empty id"), 12_345)
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

/// Drops `store` and opens its root again -- the restart these recovery tests mean.
///
/// Since `D116` a root has one writer, so reopening while the first handle is alive is
/// refused. That is worth honouring rather than working around: a "reopen" that kept the
/// original handle live was never modelling a restart, and could not have caught a
/// recovery path that depended on state the old handle still held.
fn reopen(vfs: &Arc<dyn Vfs>, store: Store) -> Result<Store, StoreError> {
    let root = store.root().to_path_buf();
    drop(store);
    Store::open_unsourced(Arc::clone(vfs), root)
}

use crate::{Nonce, NonceError};

fn nonce(raw: &str) -> Nonce {
    Nonce::new(raw).expect("valid nonce")
}

use std::ffi::OsString;

/// The file names in namespace replica `index`'s directory for the node `RECORD`, for
/// asserting no `.tmp.<nonce>` staging orphan survives a commit.
///
/// One directory per path component since `D222`, so a one-component path is one
/// directory below the replica root -- there is no synthesized shard tier above it.
fn node_entries(vfs: &Arc<dyn Vfs>, store: &Store, index: usize) -> Vec<OsString> {
    let node_dir = namespace_root(store, index).join("RECORD");
    let mut names: Vec<OsString> = vfs
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
    let options = StoreOptions::with_nonce_source(counting_nonce_source());
    let store = Store::init(vfs.clone(), root, test_config(), options).expect("init store");
    (vfs, store)
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
