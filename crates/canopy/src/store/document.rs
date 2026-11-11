// Copyright (c) Michael Grier.
// Split from store.rs at 1303f9a.

//! Reading and writing a whole document: the non-transactional write that reaches both
//! the content store and the namespace, and the resolve-then-fetch read over them.

use super::*;
use crate::naming::Artifact;
use crate::read::{Contended, Contention, Read};

impl Store {
    /// Writes `body` as the document named `document` at `node`, without a transaction.
    ///
    /// The write is made under a nonce drawn from the store's source and claimed for it, and
    /// the nonce is returned.
    ///
    /// # It writes both trees, and journals neither (`D332`)
    ///
    /// The blob is ingested and the pointer moved in the order `D327` requires: the content
    /// is durable before anything names it. Nothing here is journalled, so that ordering is
    /// not a belt-and-braces measure -- it is the only thing preventing a dangling pointer,
    /// because there is no `prepare` for recovery to roll forward from.
    ///
    /// **What resolves a crash mid-write is the pointer quorum** (`D320`). A crash between
    /// the pointer renames leaves a minority holding the new entry, the majority holding the
    /// old one, and both naming blobs that are already durable.
    ///
    /// **A crash before the pointer moves leaves an unreferenced blob this path cannot
    /// collect.** The transactional path names its targets in `begin` so recovery can find
    /// them (`D329`); this one announces nothing, so an interrupted write's content survives
    /// until the next write to the same document sweeps it as a set difference. That is a
    /// space leak rather than a corruption -- nothing names it -- and it is the clearest
    /// thing the journal buys that this path does without.
    pub fn write_document_replicated(
        &self,
        node: impl Into<NodePath>,
        document: &DocumentName,
        body: &DocumentBody,
    ) -> Result<Nonce, StoreError> {
        let _writer = self.writable()?;
        let claim = self.claim_fresh_nonce()?;
        self.write_document_replicated_as(node, document, body, claim.nonce())?;
        let nonce = claim.nonce().clone();
        drop(claim);
        self.settle_log();
        Ok(nonce)
    }

    /// [`write_document_replicated`](Store::write_document_replicated) under `nonce`, which
    /// the caller has claimed or, in a test, chosen.
    pub(crate) fn write_document_replicated_as(
        &self,
        node: impl Into<NodePath>,
        document: &DocumentName,
        body: &DocumentBody,
        nonce: &Nonce,
    ) -> Result<(), StoreError> {
        let _writer = self.writable()?;
        // One modification from ingest through collection (`D335`).
        let _modifying = self.begin_modification();
        let node = node.into();

        // Before the blob is ingested: a write the namespace will refuse must not leave a
        // blob nothing will ever name.
        for index in 0..self.genesis.pointer_replica_count {
            self.check_namespace_tree_collision(index, &node, document)?;
        }

        // The content store first, and durable before anything names it (`D327`).
        let name = self.ingest_blob(&node, document, Artifact::Body, body.as_bytes(), nonce)?;
        self.write_body_pointer(&node, document, name.digest(), nonce)?;

        // Without this every overwrite would strand the version it replaced, on the success
        // path rather than only after a crash (`D328`). The keep-set is read from the
        // namespace, which this write has just settled for its own document.
        self.collect_document_blobs(&node, document)?;
        self.report_written(&node, document, body);
        Ok(())
    }

    /// Reads the document named `document` at `node` through the namespace: the pointer
    /// quorum names a blob, and that blob is verified against the digest in its own name
    /// (`D316`, `D320`).
    ///
    /// There is no vote over bodies here. A blob either matches its digest or does not,
    /// decided by examining one copy alone, so a bad replica is identified rather than
    /// outvoted and [`read_blob`](Store::read_blob) falls through to the next one.
    ///
    /// # The read reports damage; it does not repair it (`D339`)
    ///
    /// A replica found missing or rotted on the way to a good copy is recorded on the
    /// store's repair backlog, which
    /// [`repair_reported_damage`](Store::repair_reported_damage) drains from the write side.
    /// Reading stays free of writes -- and of the nonce a write would need (`D61`) -- while
    /// `D317`'s probe still reaches something that acts on it.
    ///
    /// # Absence is a candidate, not the absence of one (`D338`)
    ///
    /// [`Read::Absent`] means a **majority** of pointer replicas hold no entry. Requiring
    /// unanimity looked safer and was not: mid-create, one replica holds the new entry and
    /// the rest hold nothing, and a rule that only counts replicas holding *pointers* finds
    /// no majority and calls a healthy store unreadable. The replicas holding nothing are
    /// holding the document's previous state, exactly as a mid-overwrite majority holds the
    /// previous pointer -- so they are counted, and the answer stays monotonic: absent,
    /// absent, then found, never back again.
    ///
    /// **A replica that will not parse still poisons the vote.** That is damage rather than a
    /// state, so a namespace with any rejected replica is an error however the rest fall.
    /// Answering "no such document" off the back of an unreadable namespace would turn a
    /// recoverable fault into silent data loss -- the same distinction the blob collector
    /// draws before it deletes anything, and for the same reason.
    ///
    /// # Being outrun is an outcome, not a fault (`D336`)
    ///
    /// Two things a live writer does leave this read with nothing, and both report
    /// [`Read::Contended`] rather than an error, because **nothing is retried here**: when to
    /// read again, when to give up, what delay to use and how cancellation reaches the loop
    /// are the caller's, and [`read_settled`](crate::read_settled) is where such a loop
    /// belongs.
    ///
    /// [`Contention::Superseded`] is the two-step race: resolving the name and fetching the
    /// content are separate, and `D328` has a write collect what it superseded, so a write
    /// landing between them sweeps the version this read resolved. Telling that apart from
    /// real loss takes exactly two resolutions -- if the second names the same content,
    /// nothing swept it and the blob is genuinely gone, so the error stands.
    ///
    /// [`Contention::Undetermined`] is the namespace itself refusing to settle: replicas
    /// present, none commanding a majority, and not enough absent to carry the vote either.
    /// Re-resolving here would not help -- there is no second question to ask, only the same
    /// one later -- so it returns at once and the caller's loop is the whole of the retry.
    pub fn read_document(
        &self,
        node: impl Into<NodePath>,
        document: &DocumentName,
    ) -> Result<Read, StoreError> {
        let node = node.into();
        let mut resolved: Option<ContentDigest> = None;

        // Two attempts, which is not a retry budget: it is what it takes to tell the two
        // causes apart. The second resolution is the question "did the namespace move?",
        // and asking it twice more would only be answering a different question -- whether
        // to keep waiting -- which is the caller's.
        for _ in 0..2 {
            let read = match self.read_pointer(&node, document) {
                Ok(read) => read,
                // No pointer commanded a majority. Absence might still have: count the
                // replicas holding nothing against the same threshold the pointer vote used.
                Err(StoreError::PointerUnavailable {
                    rejected: 0,
                    missing,
                    disagreeing,
                    ..
                }) => {
                    // Nothing was rejected, so every replica is accounted for by one tally
                    // or the other and their sum is the replica count.
                    let replicas = missing + disagreeing;
                    if missing >= namespace::pointer_majority(replicas) {
                        return Ok(Read::Absent);
                    }
                    return Ok(Read::Contended(Contended {
                        cause: Contention::Undetermined {
                            present: disagreeing,
                            missing,
                        },
                        modifications: self.modifications(),
                    }));
                }
                Err(other) => return Err(other),
            };
            let target = read.pointer().target();
            let digest = target.name().digest();

            match self.read_blob(&target) {
                Ok(blob) => {
                    if !blob.was_first_choice() {
                        self.note_damaged_blob(target);
                    }
                    return Ok(Read::Found(DocumentBody::new(blob.into_bytes())));
                }
                // Every copy gone and none rotted is what a collection leaves behind, so
                // either the namespace moved and swept the version this read resolved, or
                // the content is really lost. Resolving again is what separates them.
                Err(error @ StoreError::BlobUnavailable { corrupt: 0, .. }) => {
                    if resolved.replace(digest) == Some(digest) {
                        // Standing still, so nothing swept it: the loss is real.
                        return Err(error);
                    }
                }
                Err(other) => return Err(other),
            }
        }

        Ok(Read::Contended(Contended {
            cause: Contention::Superseded {
                resolved: resolved.expect("only the superseded path reaches here"),
            },
            modifications: self.modifications(),
        }))
    }
}
