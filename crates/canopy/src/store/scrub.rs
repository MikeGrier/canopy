// Copyright (c) Michael Grier.
// Split from store.rs at 1303f9a.
//! The content-store scrub `CVS-18` adds ([`Store::scrub_content`]), which verifies the
//! namespace and the blobs it names and repairs what fails (`D331`).

use super::repair::RepairOutcome;
use super::*;
use crate::naming::BlobRef;

/// The catalogue a content-store scrub produces (`D331`).
///
/// A count of what was examined and a list of what was acted on. The repairs are recorded
/// per subject rather than summarised, because `D326` requires a remediation to be
/// attributable: "twelve replicas rebuilt" does not tell an operator whether one disk is
/// dying or twelve documents each lost a copy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContentScrubReport {
    documents: usize,
    whole: usize,
    pointer_repairs: Vec<(String, RepairOutcome)>,
    blob_repairs: Vec<(String, RepairOutcome)>,
    stale: Vec<(String, Vec<usize>)>,
    unrepairable: Vec<String>,
}

impl ContentScrubReport {
    /// How many documents the namespace held.
    pub fn documents(&self) -> usize {
        self.documents
    }

    /// How many needed nothing: every pointer replica agreed -- none rejected, missing or
    /// stale -- and every blob replica verified.
    pub fn whole(&self) -> usize {
        self.whole
    }

    /// Namespace entries rebuilt, by `node/document`.
    pub fn pointer_repairs(&self) -> &[(String, RepairOutcome)] {
        &self.pointer_repairs
    }

    /// Blobs re-replicated, by blob path.
    pub fn blob_repairs(&self) -> &[(String, RepairOutcome)] {
        &self.blob_repairs
    }

    /// Documents with pointer replicas that parse but trail the majority, by
    /// `node/document`, with the stale replica indices.
    ///
    /// Reported, never repaired (`D320`): a stale replica converges when the transaction
    /// that moved it completes. A non-transactional write interrupted part-way has no such
    /// transaction, so staleness that persists across scrubs is worth an operator's look.
    pub fn stale(&self) -> &[(String, Vec<usize>)] {
        &self.stale
    }

    /// Documents the scrub could not restore, by `node/document`.
    ///
    /// A pointer with no readable majority, or a blob with no intact copy anywhere. Neither
    /// is repairable and canopy invents neither: a blob's content cannot be reconstructed from
    /// copies that all fail the digest in its name, and a pointer no majority agrees on names
    /// nothing the store can act on (`D319`).
    pub fn unrepairable(&self) -> &[String] {
        &self.unrepairable
    }

    /// Whether the scrub found the store fully redundant and current.
    pub fn is_clean(&self) -> bool {
        self.pointer_repairs.is_empty()
            && self.blob_repairs.is_empty()
            && self.stale.is_empty()
            && self.unrepairable.is_empty()
    }
}

/// What a backlog drain did (`D339`).
///
/// Separate from [`ContentScrubReport`] because the two answer different questions: a scrub
/// reports on the whole store, and this reports only on blobs a read had already found
/// wanting. Counting a targeted repair as though it were a sweep would tell an operator the
/// store had been examined when it had not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BacklogRepairReport {
    examined: usize,
    already_whole: usize,
    repairs: Vec<(String, RepairOutcome)>,
    unrepairable: Vec<String>,
}

impl BacklogRepairReport {
    /// How many suspect blobs the drain took off the backlog.
    pub fn examined(&self) -> usize {
        self.examined
    }

    /// How many needed nothing by the time the drain reached them.
    ///
    /// Routine rather than surprising: a scrub may have run, the document may have been
    /// overwritten, or the blob may have been collected since the read that reported it.
    pub fn already_whole(&self) -> usize {
        self.already_whole
    }

    /// Blobs whose replicas were rebuilt, by blob path.
    pub fn repairs(&self) -> &[(String, RepairOutcome)] {
        &self.repairs
    }

    /// Blobs with no intact copy left to rebuild from, by blob path.
    ///
    /// canopy invents no content (`D319`), so these are reported and left alone.
    pub fn unrepairable(&self) -> &[String] {
        &self.unrepairable
    }

    /// Whether the drain found nothing to do.
    pub fn is_clean(&self) -> bool {
        self.repairs.is_empty() && self.unrepairable.is_empty()
    }
}

impl Store {
    /// Verifies every namespace entry and every blob they name, repairing what fails
    /// verification (`D316`, `D320`, `D331`).
    ///
    /// # Call this regularly. It is an obligation, not a utility (`D340`)
    ///
    /// **A store that is never scrubbed loses redundancy silently.** Nothing else in canopy is
    /// exhaustive: reads probe whatever they happen to touch (`D317`) and the repair backlog
    /// they feed is bounded and lossy by design (`D339`, `D341`), so a replica belonging to a
    /// document nobody reads is rebuilt by this pass or by nothing. The failure is quiet --
    /// every read keeps succeeding from a surviving copy, right up until the last one is gone.
    ///
    /// canopy cannot schedule this for you. It is a library with no clock, no scheduler and no
    /// thread of its own to spare (`D61`), so **the cadence is the embedder's to choose and
    /// the embedder's to honour.** What canopy owes in return is to say so plainly, which is
    /// what this paragraph is.
    ///
    /// Pick a period against how long you are willing to sit on an undetected bad replica,
    /// and treat the pass as maintenance: it walks every document, so its cost scales with
    /// the store rather than with what changed.
    ///
    /// # Why this walks the namespace rather than the blob tree
    ///
    /// The namespace is what makes a blob reachable, so walking it visits exactly the content
    /// a reader can ask for -- and visits each blob under the name that says what its bytes
    /// must hash to, which is what verification needs. Walking the blob tree instead would
    /// visit unreachable blobs too, and an unreachable blob is not a correctness problem
    /// (`D328`, `D329` collect them from records rather than from a scan).
    ///
    /// # Exhaustive, unlike a read
    ///
    /// Each blob is checked with [`Store::verify_blob`], which examines every replica, rather
    /// than with a read, which stops at the first copy that verifies. A scrub that stopped
    /// early would leave exactly the replicas a read already covers.
    ///
    /// # What it will not do
    ///
    /// It never repairs a **stale** pointer replica (`D320`), and it never invents content for
    /// a blob whose every copy fails verification -- that is reported, not guessed. The store
    /// must be quiescent: this reads and rewrites replicas while assuming no transaction is
    /// moving them.
    ///
    /// Each repair is made under its own nonce, drawn from the store's source and claimed for
    /// the pass; the nonces are returned with the report, in the order they were used.
    pub fn scrub_content(&self) -> Result<(ContentScrubReport, Vec<Nonce>), StoreError> {
        self.with_claimed_nonces(|mint| self.scrub_content_minting(mint))
    }

    fn scrub_content_minting(
        &self,
        mint: &mut dyn FnMut() -> Result<Nonce, StoreError>,
    ) -> Result<ContentScrubReport, StoreError> {
        let mut report = ContentScrubReport::default();

        // Settles a pending publication before the walk, so the documents listed and the
        // pointers judged stale are the ones the settled store holds.
        drop(self.writable()?);
        for (node, document) in self.list_documents()? {
            // Per document, not per scrub: a pass over a large store must not hold every
            // other writer off for its whole duration. Each document is checked and repaired
            // atomically; the pass as a whole is not a snapshot.
            let _writer = self.writable()?;
            report.documents += 1;
            let subject = format!("{node}/{}", document.as_str());

            let read = match self.read_pointer(&node, &document) {
                Ok(read) => read,
                // No majority: nothing to rebuild from, and canopy never invents a pointer.
                Err(StoreError::PointerUnavailable { .. }) => {
                    report.unrepairable.push(subject);
                    continue;
                }
                Err(other) => return Err(other),
            };

            let pointer_intact = read.rejected().is_empty() && read.missing().is_empty();
            if !pointer_intact {
                let outcome = self.repair_pointer_as(&node, &document, &read, &mint()?)?;
                report.pointer_repairs.push((subject.clone(), outcome));
            }
            let pointer_current = read.stale().is_empty();
            if !pointer_current {
                report.stale.push((subject.clone(), read.stale().to_vec()));
            }

            let reference = read.pointer().target();
            match self.restore_blob(&reference, mint)? {
                None => {
                    if pointer_intact && pointer_current {
                        report.whole += 1;
                    }
                }
                Some(None) => report.unrepairable.push(subject),
                Some(Some(outcome)) => report
                    .blob_repairs
                    .push((reference.relative_path(), outcome)),
            }
        }

        Ok(report)
    }

    /// Examines every replica of `reference` and rebuilds the ones that fail.
    ///
    /// `Ok(None)` when the blob was already whole, `Ok(Some(None))` when no copy survives to
    /// rebuild from, and `Ok(Some(Some(outcome)))` when replicas were restored.
    ///
    /// Shared by the exhaustive scrub and the backlog drain so the two cannot diverge on what
    /// repairing a blob means -- they differ only in **which** blobs they visit.
    fn restore_blob(
        &self,
        reference: &BlobRef,
        mint: &mut dyn FnMut() -> Result<Nonce, StoreError>,
    ) -> Result<Option<Option<RepairOutcome>>, StoreError> {
        let verification = self.verify_blob(reference)?;
        if verification.is_whole() {
            return Ok(None);
        }

        // The source must be a copy that verified. Reading gives us one, and gives it
        // from whichever replica is intact rather than from a vote (`CVS-9`).
        let source = match self.read_blob(reference) {
            Ok(read) => read.into_bytes(),
            Err(StoreError::BlobUnavailable { .. }) => return Ok(Some(None)),
            Err(other) => return Err(other),
        };
        let outcome = self.repair_blob_as(
            reference,
            &source,
            verification.missing(),
            verification.corrupt(),
            &mint()?,
        )?;
        Ok(Some(Some(outcome)))
    }

    /// Repairs the blobs reads have reported damaged, emptying the backlog (`D339`).
    ///
    /// This is what makes `D317`'s claim -- that rotation turns every read into a probe --
    /// pay off: a rotted replica is rebuilt shortly after the next read of any document
    /// resident on it, rather than at the next whole-store scrub.
    ///
    /// # Why the reader does not do this itself
    ///
    /// Repairing is a write and a write needs a nonce. Drawing one on every read that found
    /// damage would put a write on the latency path of every read. Reads record; a writer
    /// repairs.
    ///
    /// # It re-examines rather than trusting the finding
    ///
    /// The backlog carries blob identities, not the read's tally of bad replicas, so this
    /// establishes afresh what is wrong before touching anything. It therefore repairs
    /// **every** damaged replica rather than only those the read happened to try on its way
    /// to a good copy, and a blob that has been repaired or collected since the read costs
    /// one verification and nothing else.
    ///
    /// The store must be quiescent, for the same reason [`Store::scrub_content`] requires it.
    /// Each repair is made under its own nonce, drawn from the store's source and claimed for
    /// the drain; the nonces are returned with the report, in the order they were used.
    pub fn repair_reported_damage(&self) -> Result<(BacklogRepairReport, Vec<Nonce>), StoreError> {
        self.with_claimed_nonces(|mint| self.drain_reported_damage(mint))
    }

    /// [`repair_reported_damage`](Store::repair_reported_damage) under nonces the test
    /// supplies, unclaimed.
    #[cfg(test)]
    pub(crate) fn repair_reported_damage_with(
        &self,
        mut nonce_source: impl FnMut() -> Nonce,
    ) -> Result<BacklogRepairReport, StoreError> {
        self.drain_reported_damage(&mut || Ok(nonce_source()))
    }

    /// [`repair_reported_damage`](Store::repair_reported_damage) over a minter that may refuse,
    /// so a nonce that cannot be claimed fails the drain instead of being used.
    fn drain_reported_damage(
        &self,
        mint: &mut dyn FnMut() -> Result<Nonce, StoreError>,
    ) -> Result<BacklogRepairReport, StoreError> {
        let _writer = self.writable()?;
        let suspects = self
            .repair_backlog
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();

        let mut report = BacklogRepairReport::default();
        let mut suspects = suspects.into_iter();
        while let Some(reference) = suspects.next() {
            report.examined += 1;
            let restored = match self.restore_blob(&reference, mint) {
                Ok(restored) => restored,
                Err(error) => {
                    // The backlog was taken whole, so a failure part-way would otherwise
                    // forget this suspect and every one not yet reached. Put them back for
                    // the next drain.
                    let mut backlog = self
                        .repair_backlog
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    backlog.note(reference);
                    for remaining in suspects {
                        backlog.note(remaining);
                    }
                    return Err(error);
                }
            };
            match restored {
                // Repaired by a scrub, collected, or the damage was transient.
                None => report.already_whole += 1,
                Some(None) => report.unrepairable.push(reference.relative_path()),
                Some(Some(outcome)) => {
                    report.repairs.push((reference.relative_path(), outcome));
                }
            }
        }
        Ok(report)
    }
}
