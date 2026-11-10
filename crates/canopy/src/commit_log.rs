// Copyright (c) Michael Grier.

//! The append-only NDJSON commit log for the canopy store's two-phase commit (D53).
//!
//! A single file at the **store root** -- beside the [`.shire`](crate::MARKER_NAME)
//! marker and above the `p{i}/` partitions, because it is the cross-partition
//! coordinator's journal, not the property of any one partition. The log is **one
//! JSON object per line** (NDJSON): each [`append`](CommitLog::append) writes one
//! record's compact JSON followed by a single `\n`, so every complete record is a
//! self-delimited line.
//!
//! **Torn-tail discard.** A crash mid-append can leave a partial trailing line with
//! no terminating `\n`. On [`read`](CommitLog::read) only whole lines up to the last
//! newline are authoritative; any bytes after the last `\n` (a torn final line) are
//! discarded. An absent or empty log therefore reads as zero records, and a crash
//! during the very first append reads as zero records.
//!
//! Every record carries a transaction id (a caller-supplied [`Nonce`], D61) and a
//! `type`; the three types are `begin`, `prepare`, and `end` ([`LogRecord`]). A
//! `prepare` carries the transaction's [`PreparedSet`] -- the D53 commit point's
//! payload -- so the log alone describes what a committed transaction changes.
//!
//! **The log names bodies by digest, never by value (`CVS-14`, `D327`).** A `prepare`
//! records each target and the *digest* of the body the transaction places there; the
//! bytes themselves live in the content store, durable before the `prepare` that names
//! them was written. This is not merely a size optimisation -- it is what makes the log
//! a *journal of intent* rather than a second copy of the store, and the type system
//! carries the distinction: a [`PreparedSwap`] has no body to hand out, so no recovery
//! path can accidentally come to depend on one.
//!
//! **Scope (TXN-1).** This module is only the log *primitive*: the record format,
//! append (with fsync), and torn-tail-tolerant read/parse. The two-phase apply path
//! that writes `prepare`/`end` around the staged renames (TXN-2), the quiescence
//! truncation (TXN-3), and the recovery replay (TXN-4) are layered on top of it.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use acadia::{Vfs, VfsError, VfsErrorKind};
use serde::{Deserialize, Serialize};

use crate::body::ContentDigest;
use crate::name::{DocumentName, NodePath, Nonce};
use crate::transaction::{Precondition, Swap};

/// The commit log's file name at the store root, beside the
/// [`.shire`](crate::MARKER_NAME) marker and above the `p{i}/` partitions (D53).
pub(crate) const LOG_NAME: &str = ".commit-log";

/// One record in the D53 commit log: a phase transition tagged with its transaction.
///
/// The three variants are the D53 phases -- [`Begin`](LogRecord::Begin) opens a
/// transaction, [`Prepare`](LogRecord::Prepare) hardens its full [`SwapSet`] at the
/// commit point, and [`End`](LogRecord::End) records completion. Each carries the
/// caller-supplied transaction id ([`Nonce`], D61); canopy mints none of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LogRecord {
    /// A transaction is opened. Staging (LAY-3) proceeds non-destructively after this;
    /// a crash here commits nothing.
    Begin {
        /// The transaction being opened.
        txn: Nonce,
        /// The `(node, document)` pairs this transaction is about to ingest blobs for
        /// (`D329`).
        ///
        /// Recorded so a rollback can find what an interrupted staging phase left in the
        /// content store. It names the *targets*, never the blob names: a rollback that
        /// deleted by name would delete a live blob whenever the abandoned write happened
        /// to carry the content the namespace already points at.
        targets: Vec<(NodePath, DocumentName)>,
    },
    /// The transaction's prepared set is hardened. Fsyncing the log after appending
    /// this record is D53's single commit / linearization point.
    Prepare { txn: Nonce, prepared: PreparedSet },
    /// Every swap has landed; the transaction is complete.
    End { txn: Nonce },
}

impl LogRecord {
    /// The transaction id this record belongs to.
    pub(crate) fn txn(&self) -> &Nonce {
        match self {
            LogRecord::Begin { txn, .. } | LogRecord::End { txn } => txn,
            LogRecord::Prepare { txn, .. } => txn,
        }
    }

    fn to_wire(&self) -> WireRecord {
        match self {
            LogRecord::Begin { txn, targets } => WireRecord::Begin {
                txn: txn.as_str().to_owned(),
                targets: targets
                    .iter()
                    .map(|(node, document)| WireTarget {
                        node: node.to_string(),
                        document: document.as_str().to_owned(),
                    })
                    .collect(),
            },
            LogRecord::Prepare { txn, prepared } => WireRecord::Prepare {
                txn: txn.as_str().to_owned(),
                swaps: prepared.swaps().iter().map(WireSwap::from_swap).collect(),
                remove_nodes: prepared
                    .node_removals()
                    .iter()
                    .map(NodePath::to_string)
                    .collect(),
            },
            LogRecord::End { txn } => WireRecord::End {
                txn: txn.as_str().to_owned(),
            },
        }
    }

    fn from_wire(wire: WireRecord) -> Result<LogRecord, CommitLogError> {
        match wire {
            WireRecord::Begin { txn, targets } => Ok(LogRecord::Begin {
                txn: parse_txn(&txn)?,
                targets: targets
                    .into_iter()
                    .map(WireTarget::into_target)
                    .collect::<Result<_, _>>()?,
            }),
            WireRecord::Prepare {
                txn,
                swaps,
                remove_nodes,
            } => {
                let txn = parse_txn(&txn)?;
                let swaps: Vec<PreparedSwap> = swaps
                    .into_iter()
                    .map(WireSwap::into_swap)
                    .collect::<Result<_, _>>()?;
                let removals: Vec<NodePath> = remove_nodes
                    .iter()
                    .map(|raw| {
                        NodePath::parse(raw).map_err(|e| {
                            CommitLogError::Malformed(format!("invalid node removal: {e}"))
                        })
                    })
                    .collect::<Result<_, _>>()?;
                let prepared = PreparedSet::parsed(swaps, removals)?;
                Ok(LogRecord::Prepare { txn, prepared })
            }
            WireRecord::End { txn } => Ok(LogRecord::End {
                txn: parse_txn(&txn)?,
            }),
        }
    }
}

/// What a [`PreparedSwap`] does to its target, as the `prepare` record carries it
/// (`CVS-14`).
///
/// The digest stands exactly where [`Effect::Write`](crate::transaction::Effect)'s body
/// stands in the in-memory [`Swap`]: same position, same meaning, naming the content
/// instead of carrying it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreparedEffect {
    /// Place the body with this digest at the target. The blob is already durable.
    Write(ContentDigest),
    /// Remove the target. A removal names no content, so it carries no digest.
    Remove,
}

/// One document change as the `prepare` record carries it: the target, the digest of
/// the body the transaction places there, and the precondition that admitted it
/// (`CVS-14`, `D327`).
///
/// **This type deliberately cannot yield a body.** A [`Swap`] carries the bytes because
/// the live apply path must write them; by the time a transaction reaches its `prepare`
/// those bytes are already durable in the content store, so the record names them and
/// nothing more. Recovery is handed this type rather than a [`Swap`], which is what
/// makes "recovery never replays bodies out of the log" a property of the code rather
/// than a convention someone must remember.
///
/// The precondition is recorded but never re-evaluated: it passed before the `prepare`
/// was written, and D53 roll-forward re-applies without re-checking it. It is kept for
/// the forensic record -- a log line that says *why* a transaction was allowed to
/// proceed is worth the two fields it costs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedSwap {
    node: NodePath,
    document: DocumentName,
    effect: PreparedEffect,
    precondition: Precondition,
}

impl PreparedSwap {
    /// The prepared form of a write, naming the blob `digest` that is already durable.
    ///
    /// `digest` is the one the content store computed over the bytes it wrote, not one
    /// re-derived from the in-memory body. There is therefore only ever a single digest
    /// in play for a given write -- the blob's name, the pointer's target, and this
    /// record are the same value by construction rather than by three agreeing
    /// computations.
    pub(crate) fn writing(swap: &Swap, digest: ContentDigest) -> PreparedSwap {
        PreparedSwap {
            node: swap.node().clone(),
            document: swap.document().clone(),
            effect: PreparedEffect::Write(digest),
            precondition: swap.precondition(),
        }
    }

    /// The prepared form of a removal, which names no content.
    pub(crate) fn removing(swap: &Swap) -> PreparedSwap {
        PreparedSwap {
            node: swap.node().clone(),
            document: swap.document().clone(),
            effect: PreparedEffect::Remove,
            precondition: swap.precondition(),
        }
    }

    /// The node holding this swap's target document.
    pub(crate) fn node(&self) -> &NodePath {
        &self.node
    }

    /// The target document's name within its node.
    pub(crate) fn document(&self) -> &DocumentName {
        &self.document
    }

    /// The digest of the body this swap places, or `None` for a removal.
    pub(crate) fn digest(&self) -> Option<ContentDigest> {
        match self.effect {
            PreparedEffect::Write(digest) => Some(digest),
            PreparedEffect::Remove => None,
        }
    }

    /// Whether this swap removes its target rather than writing it.
    pub(crate) fn is_removal(&self) -> bool {
        matches!(self.effect, PreparedEffect::Remove)
    }

    /// The precondition that admitted this swap (D54), recorded but never re-evaluated.
    pub(crate) fn precondition(&self) -> Precondition {
        self.precondition
    }
}

/// A transaction's changes as the `prepare` record carries them: the [`PreparedSwap`]s
/// and the nodes the transaction removes (`CVS-14`, `BG-6`).
///
/// The digest-carrying counterpart of [`SwapSet`], and the payload D53's commit point
/// hardens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedSet {
    swaps: Vec<PreparedSwap>,
    node_removals: Vec<NodePath>,
}

impl PreparedSet {
    /// The prepared set the store is about to harden.
    ///
    /// Infallible because the caller built it from an already-validated [`SwapSet`]:
    /// distinctness of targets was settled at that set's construction, and nothing here
    /// can reintroduce a duplicate. Records arriving from disk take
    /// [`parsed`](PreparedSet::parsed) instead, which is where the checking belongs --
    /// the wire is the trust boundary, the in-process call is not.
    pub(crate) fn new(swaps: Vec<PreparedSwap>, node_removals: Vec<NodePath>) -> PreparedSet {
        PreparedSet {
            swaps,
            node_removals,
        }
    }

    /// The prepared set a record read off disk describes, re-validated (D56).
    ///
    /// A duplicate target is refused rather than applied: the two entries disagree about
    /// what the document ends up holding, and picking one silently would let a corrupted
    /// or forged record drive recovery to an outcome no transaction ever committed.
    fn parsed(
        swaps: Vec<PreparedSwap>,
        node_removals: Vec<NodePath>,
    ) -> Result<PreparedSet, CommitLogError> {
        let mut seen: HashSet<(&NodePath, &str)> = HashSet::with_capacity(swaps.len());
        for swap in &swaps {
            if !seen.insert((&swap.node, swap.document.as_str())) {
                return Err(CommitLogError::Malformed(format!(
                    "prepare names {}/{} twice",
                    swap.node,
                    swap.document.as_str()
                )));
            }
        }
        let mut seen_nodes: HashSet<&NodePath> = HashSet::with_capacity(node_removals.len());
        for path in &node_removals {
            if !seen_nodes.insert(path) {
                return Err(CommitLogError::Malformed(format!(
                    "prepare removes node {path} twice"
                )));
            }
        }
        drop(seen);
        drop(seen_nodes);
        Ok(PreparedSet {
            swaps,
            node_removals,
        })
    }

    /// The document changes, in the order the transaction recorded them.
    pub(crate) fn swaps(&self) -> &[PreparedSwap] {
        &self.swaps
    }

    /// The nodes this transaction removes (`BG-6`).
    pub(crate) fn node_removals(&self) -> &[NodePath] {
        &self.node_removals
    }
}

fn parse_txn(raw: &str) -> Result<Nonce, CommitLogError> {
    Nonce::new(raw).map_err(|e| CommitLogError::Malformed(format!("invalid transaction id: {e}")))
}

/// The on-disk wire shape of a [`LogRecord`], internally tagged by `type` so every
/// line is `{"type":"begin"|"prepare"|"end",...}`. canopy owns this representation
/// (Design Autonomy): the value types stay serde-free and are re-validated on read.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum WireRecord {
    Begin {
        txn: String,
        /// The targets this transaction will ingest blobs for (`D329`).
        ///
        /// Omitted when empty, so a record written before rollback collection existed
        /// still reads -- as a transaction with nothing to collect, which is exactly what
        /// such a record described. Unlike `CVS-14`'s body-to-digest change, absent here
        /// is the honest reading rather than a silent lie, so this needs no version bump.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        targets: Vec<WireTarget>,
    },
    Prepare {
        txn: String,
        swaps: Vec<WireSwap>,
        /// Nodes this transaction removes (`BG-6`).
        ///
        /// Omitted when empty, so a record written before node removals existed still
        /// reads as the no-removals transaction it was.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        remove_nodes: Vec<String>,
    },
    End {
        txn: String,
    },
}

/// The wire shape of one [`PreparedSwap`]: the `(node, document)` target, the digest of
/// the body being placed, and the swap's precondition (D54). A digest-conditional swap
/// carries its expected digest in `expected`; a conditional-create swap sets
/// `expect_absent`; an unconditional swap sets neither. A **removal** sets `remove` and
/// carries no `digest`.
///
/// `remove` and `digest` are separate optional fields rather than one tagged union so a
/// record written before removals existed still reads: an absent `remove` is a write,
/// which is what every existing record is.
///
/// **`digest` replaced a `body` field carrying the whole document (`CVS-14`).** Both are
/// optional and absent on a removal, so a stale record would deserialize into a
/// plausible-looking write with no digest rather than failing loudly. That is precisely
/// why the store format version was bumped: a store predating this change is refused at
/// `open`, before any log is read (`D325`), so this parser is never shown one.
#[derive(Serialize, Deserialize)]
struct WireSwap {
    node: String,
    document: String,
    /// Base16 of the placed body's [`ContentDigest`], as [`ContentDigest::to_hex`]
    /// renders it. Hex rather than a byte array because serde encodes `Vec<u8>` as a
    /// JSON array of 32 integers -- four times the bytes and unreadable in a log a human
    /// is expected to read during an incident.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "is_false")]
    expect_absent: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    remove: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// The wire shape of one `(node, document)` target named by a `begin` record.
#[derive(Serialize, Deserialize)]
struct WireTarget {
    node: String,
    document: String,
}

impl WireTarget {
    fn into_target(self) -> Result<(NodePath, DocumentName), CommitLogError> {
        let node = NodePath::parse(&self.node)
            .map_err(|e| CommitLogError::Malformed(format!("invalid begin-target node: {e}")))?;
        let document = DocumentName::new(&self.document).map_err(|e| {
            CommitLogError::Malformed(format!("invalid begin-target document: {e}"))
        })?;
        Ok((node, document))
    }
}

impl WireSwap {
    fn from_swap(swap: &PreparedSwap) -> WireSwap {
        let expected = match swap.precondition() {
            Precondition::ExpectedDigest(digest) => Some(digest.as_bytes().to_vec()),
            Precondition::Unconditional | Precondition::ExpectedAbsent => None,
        };
        WireSwap {
            node: swap.node().to_string(),
            document: swap.document().as_str().to_owned(),
            digest: swap.digest().map(|digest| digest.to_hex()),
            expected,
            expect_absent: matches!(swap.precondition(), Precondition::ExpectedAbsent),
            remove: swap.is_removal(),
        }
    }

    fn into_swap(self) -> Result<PreparedSwap, CommitLogError> {
        let node = NodePath::parse(&self.node)
            .map_err(|e| CommitLogError::Malformed(format!("invalid swap node: {e}")))?;
        let document = DocumentName::new(&self.document)
            .map_err(|e| CommitLogError::Malformed(format!("invalid swap document: {e}")))?;

        let expected = match self.expected {
            None => None,
            Some(bytes) => {
                let bytes: [u8; ContentDigest::LEN] = bytes.try_into().map_err(|v: Vec<u8>| {
                    CommitLogError::Malformed(format!(
                        "expected-digest is {} bytes, want {}",
                        v.len(),
                        ContentDigest::LEN
                    ))
                })?;
                Some(ContentDigest::from_bytes(bytes))
            }
        };

        if self.expect_absent && expected.is_some() {
            return Err(CommitLogError::Malformed(
                "swap carries both an expected-digest and expect-absent precondition".to_owned(),
            ));
        }

        let precondition = match (expected, self.expect_absent) {
            (Some(digest), false) => Precondition::ExpectedDigest(digest),
            (None, true) => Precondition::ExpectedAbsent,
            (None, false) => Precondition::Unconditional,
            // Ruled out above; spelled out so the match is exhaustive without a wildcard
            // that would swallow a future fourth combination.
            (Some(_), true) => unreachable!("rejected above as a contradictory precondition"),
        };

        if self.remove {
            // A removal names no content, so a digest is a contradiction rather than a
            // harmless extra: it would mean the record was written by something that
            // did not agree with this build about what a removal is.
            if self.digest.is_some() {
                return Err(CommitLogError::Malformed(
                    "removal swap carries a body digest".to_owned(),
                ));
            }
            if self.expect_absent {
                return Err(CommitLogError::Malformed(
                    "removal swap carries an expect-absent precondition, which no removal can \
                     usefully hold"
                        .to_owned(),
                ));
            }
            return Ok(PreparedSwap {
                node,
                document,
                effect: PreparedEffect::Remove,
                precondition,
            });
        }

        // A write with no digest names nothing, so there is no body for recovery to place.
        // Before `CVS-14` the analogous field was a body that defaulted to empty when
        // absent; defaulting here would invent a digest -- and an invented digest is a
        // pointer to a blob that was never written.
        let digest = match self.digest {
            Some(hex) => ContentDigest::from_hex(&hex).ok_or_else(|| {
                CommitLogError::Malformed(format!("invalid body digest: {hex:?}"))
            })?,
            None => {
                return Err(CommitLogError::Malformed(
                    "write swap carries no body digest".to_owned(),
                ));
            }
        };

        Ok(PreparedSwap {
            node,
            document,
            effect: PreparedEffect::Write(digest),
            precondition,
        })
    }
}
/// The append-only NDJSON commit log at a store root (D53).
///
/// A thin handle over the [`Vfs`] and the log's path; it holds no in-memory buffer,
/// so every [`append`](CommitLog::append) and [`read`](CommitLog::read) goes to the
/// backing file. Construct one with [`CommitLog::new`] or via `Store::commit_log`.
pub(crate) struct CommitLog {
    vfs: Arc<dyn Vfs>,
    path: PathBuf,
    durability: Arc<LogDurability>,
}

/// What is known to be durable about the log, shared by every handle to one open store.
///
/// Each starts unknown at open and is learned by a sync that succeeded, so a failed sync is
/// retried by the next operation rather than forgotten.
#[derive(Debug, Default)]
pub(crate) struct LogDurability {
    /// The log's directory entry in the store root.
    name_synced: AtomicBool,
    /// The log being empty: set by a reset whose flush succeeded, cleared by any append.
    empty_synced: AtomicBool,
}

impl CommitLog {
    /// The commit log at `store_root`'s [`LOG_NAME`] file.
    pub(crate) fn new(vfs: Arc<dyn Vfs>, store_root: &Path) -> CommitLog {
        CommitLog {
            vfs,
            path: store_root.join(LOG_NAME),
            durability: Arc::default(),
        }
    }

    /// Shares what is known durable with other handles to the same log, so each sync is
    /// paid once for all of them rather than once per handle.
    pub(crate) fn sharing(mut self, durability: Arc<LogDurability>) -> CommitLog {
        self.durability = durability;
        self
    }

    /// The log file's path.
    ///
    /// Only the tests reach for this: the log's own operations already hold the path,
    /// and no production caller needs to name the file.
    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Appends `record` as one NDJSON line and fsyncs the log.
    ///
    /// The record is serialized to compact single-line JSON, terminated with `\n`,
    /// appended (creating the file if absent), then fsynced so the line is durable.
    /// For a `prepare`, this fsync is D53's commit point.
    ///
    /// A torn final line left by a crash mid-append is cut off first: appending after
    /// it would weld the new record onto the fragment and turn a tolerated torn tail
    /// into a malformed complete line. Until the log's own directory entry is known durable
    /// -- the first append through this handle's shared state, or any append that creates
    /// the file -- the store root is fsynced too, and a failure leaves it unknown so the next
    /// append retries.
    pub(crate) fn append(&self, record: &LogRecord) -> Result<(), CommitLogError> {
        let mut line = serde_json::to_vec(&record.to_wire()).map_err(CommitLogError::Encode)?;
        line.push(b'\n');
        let created = match self.vfs.metadata(&self.path) {
            Ok(meta) => {
                self.cut_torn_tail(meta.len)?;
                false
            }
            Err(e) if e.kind() == VfsErrorKind::NotFound => true,
            Err(e) => return Err(CommitLogError::Vfs(e)),
        };
        self.durability.empty_synced.store(false, Ordering::Release);
        self.vfs
            .append(&self.path, &line)
            .map_err(CommitLogError::Vfs)?;
        self.vfs.fsync(&self.path).map_err(CommitLogError::Vfs)?;
        if created {
            self.durability.name_synced.store(false, Ordering::Release);
        }
        if !self.durability.name_synced.load(Ordering::Acquire)
            && let Some(root) = self.path.parent()
        {
            self.vfs.fsync(root).map_err(CommitLogError::Vfs)?;
            self.durability.name_synced.store(true, Ordering::Release);
        }
        Ok(())
    }

    /// Truncates the log back to its last complete line if it ends mid-line.
    ///
    /// Only the final byte is read on the common path; the whole log is read only when
    /// that byte shows a torn tail, which is the rare post-crash case.
    fn cut_torn_tail(&self, len: u64) -> Result<(), CommitLogError> {
        if len == 0 {
            return Ok(());
        }
        let last = self
            .vfs
            .read_at(&self.path, len - 1, 1)
            .map_err(CommitLogError::Vfs)?;
        if last.as_slice() == b"\n" {
            return Ok(());
        }
        let bytes = self.vfs.read(&self.path).map_err(CommitLogError::Vfs)?;
        let keep = bytes
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |last_newline| last_newline + 1);
        self.vfs
            .truncate(&self.path, keep as u64)
            .map_err(CommitLogError::Vfs)?;
        self.vfs.fsync(&self.path).map_err(CommitLogError::Vfs)?;
        Ok(())
    }

    /// Reads every authoritative record, discarding a torn final line.
    ///
    /// An absent log reads as no records. Only whole lines up to the last `\n` are
    /// parsed; a trailing partial line (a crash mid-append) is ignored. A *complete*
    /// line that is not a valid record is corruption, not a torn tail, and is
    /// surfaced as an error rather than silently dropped (D56).
    pub(crate) fn read(&self) -> Result<Vec<LogRecord>, CommitLogError> {
        let bytes = match self.vfs.read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == VfsErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(CommitLogError::Vfs(e)),
        };
        let authoritative = match bytes.iter().rposition(|&b| b == b'\n') {
            Some(last_newline) => &bytes[..=last_newline],
            None => return Ok(Vec::new()),
        };
        let mut records = Vec::new();
        for line in authoritative.split(|&b| b == b'\n') {
            if line.is_empty() {
                continue;
            }
            let wire: WireRecord = serde_json::from_slice(line).map_err(CommitLogError::Decode)?;
            records.push(LogRecord::from_wire(wire)?);
        }
        Ok(records)
    }

    /// Empties the log at quiescence, the D53 proactive reset.
    ///
    /// At quiescence (no in-flight transaction) the log is discardable: the durable
    /// truth is the partition trees, so an empty log is the O(1)-startup marker of a
    /// clean quiesced store. This truncates the file to zero and fsyncs it -- a single
    /// `ftruncate(0)` sequenced by the D41 single writer at its quiescent moment, so no
    /// concurrent `begin` races it. Truncation is preferred over unlink to avoid
    /// directory-entry/inode churn and keep the file in place. An absent log, or an empty
    /// one already known flushed, is left untouched, so a quiescing writer that never wrote
    /// a record pays no needless fsync.
    ///
    /// An empty log *not* known flushed is flushed: an earlier truncation whose flush failed
    /// looks exactly like a clean empty log, and a crash would bring the old records back.
    pub(crate) fn reset(&self) -> Result<(), CommitLogError> {
        let empty_synced = &self.durability.empty_synced;
        match self.vfs.metadata(&self.path) {
            Ok(meta) if meta.len == 0 => {
                if !empty_synced.load(Ordering::Acquire) {
                    self.vfs.fsync(&self.path).map_err(CommitLogError::Vfs)?;
                    empty_synced.store(true, Ordering::Release);
                }
                return Ok(());
            }
            Ok(_) => {}
            Err(e) if e.kind() == VfsErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(CommitLogError::Vfs(e)),
        }
        empty_synced.store(false, Ordering::Release);
        self.vfs
            .truncate(&self.path, 0)
            .map_err(CommitLogError::Vfs)?;
        self.vfs.fsync(&self.path).map_err(CommitLogError::Vfs)?;
        empty_synced.store(true, Ordering::Release);
        Ok(())
    }
}

/// Why a commit-log operation failed.
#[derive(Debug)]
pub enum CommitLogError {
    /// The underlying filesystem operation failed.
    Vfs(VfsError),
    /// A record could not be serialized to JSON.
    Encode(serde_json::Error),
    /// A complete log line was not valid JSON of the expected shape.
    Decode(serde_json::Error),
    /// A complete log line was well-formed JSON but failed canopy's validation
    /// (an invalid transaction id, node path, expected-digest, or swap set).
    Malformed(String),
}

impl fmt::Display for CommitLogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommitLogError::Vfs(e) => write!(f, "commit log filesystem error: {e}"),
            CommitLogError::Encode(e) => write!(f, "commit log record is not encodable: {e}"),
            CommitLogError::Decode(e) => write!(f, "commit log line is not valid JSON: {e}"),
            CommitLogError::Malformed(msg) => write!(f, "commit log line is malformed: {msg}"),
        }
    }
}

impl std::error::Error for CommitLogError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CommitLogError::Vfs(e) => Some(e),
            CommitLogError::Encode(e) | CommitLogError::Decode(e) => Some(e),
            CommitLogError::Malformed(_) => None,
        }
    }
}
