# Completed checklist items

## Moved 2026-11-09 -- Fixes from the 2026-11-09 branch review (`CHECKLIST-review-fixes.md`)

Decisions are recorded in [crates/canopy/DESIGN-NOTES.md](crates/canopy/DESIGN-NOTES.md)
and [crates/acadia/DESIGN-NOTES.md](crates/acadia/DESIGN-NOTES.md) under "Review fixes,
2026-11-09".

### M1 -- Docs and test hygiene

- [x] **RF-1** -- canopy README: stop claiming "no background threads"; distinguish the
  threadless `Store` from the explicitly spawned `WriteMonitor`.
- [x] **RF-2** -- Design notes: mark decision IDs that live only in the originating
  repository (`D107`, `D222`, ...) instead of implying they are recorded here.
- [x] **RF-3** -- copilot-instructions: make the `default-members` guidance conditional;
  this workspace has none.
- [x] **RF-4** -- `canopy-lock-holder` `the_root_is_writable_again_after_the_holder_dies`
  must actually write and read back a document.
- [x] **RF-5** -- Repair the control-character corruption in `test-timers/src/tests.rs`
  doc comment (`async`, `block_on`).

### M2 -- Test-support robustness

- [x] **RF-6** -- `test-timers/tests/dev_only.rs`: detect `[dependencies.test-timers]`
  tables and renamed dependencies (`package = "test-timers"`).
- [x] **RF-7** -- `test-timers` interval test: drop the wall-clock upper bound (now on a
  paused tokio clock).

### M3 -- acadia correctness

- [x] **RF-8** -- `OsFs::lock_exclusive` on Unix: report permission errors as
  `PermissionDenied`, not `Locked`.
- [x] **RF-9** -- `OsFs::fsync` of a directory on Windows must not silently claim
  durability.
- [x] **RF-10** -- `MemFs`: syncing a directory must not make its never-synced ancestors
  durable.

### M4 -- canopy durability and recovery

- [x] **RF-11** -- Commit log: durably create the journal (sync the store root after
  first creation).
- [x] **RF-12** -- Commit log: truncate a torn tail before appending, so a later append
  cannot weld onto it.
- [x] **RF-13** -- Blob and namespace writes: sync every newly created directory into its
  parent, not only the deepest one. A revert-probe confirmed the new crash test fails
  without the fix.
- [x] **RF-14** -- Transactions: reject namespace conflicts (document vs. node, and
  intra-set) before the `prepare` record is written.
- [x] **RF-15** -- `Store::init`: take the root lock before the emptiness and marker
  checks.

### M5 -- canopy behavior

- [x] **RF-16** -- Scrub: stale pointer replicas must not count as whole / clean.
- [x] **RF-17** -- Blob ingest: replicate staged content in bounded chunks instead of
  reading the whole blob into memory.
- [x] **RF-18** -- Single writer within a process: serialize direct writes and monitor
  writes across `Store` clones.

## Moved 2026-11-09 -- Second review fixes (from `CHECKLIST-review-2.md`)

Findings from the second Claude Sonnet review of `mikegrier/first`. Review IDs in parentheses.
Decisions recorded in [crates/canopy/DESIGN-NOTES.md](crates/canopy/DESIGN-NOTES.md) under
"Second review fixes, 2026-11-09".

### M1 -- Docs and instructions

- [x] **RV-1** (M5) -- Supersede the canopy design decisions RF-15/RF-18 contradict; fix the
  monitor comments; drop the stale `store.rs` size claim.
- [x] **RV-2** (M6) -- copilot-instructions: `--no-edit` is merge-only; module splits are an
  explicit mid-milestone commit exception; remove foreign residue.
- [x] **RV-3** (L12, L13, L14) -- Correct the "sweepable" staging comments; say where decision
  IDs in public docs live; remove foreign project names from live docs. Residue in immutable
  design records was left in place with header notes.

### M2 -- acadia

- [x] **RV-4** (M3) -- `MemFs`: a renamed directory keeps its durable contents.
- [x] **RV-5** (L15, L16) -- Soften the Windows directory-fsync claim; replace unreachable
  fallbacks with a supported-targets `compile_error!`.

### M3 -- canopy correctness

- [x] **RV-6** (M1) -- Live rollback collects the blobs an aborted transaction ingested. A
  revert-probe confirmed the monitor rollback test fails without the fix.
- [x] **RV-7** (M2, L7) -- Narrow the writer gate: scrub per document; ingest stages the
  caller's stream outside the gate; FIFO ticket gate; `WriterGuard` is `!Send`.
- [x] **RV-8** (M4) -- Re-export `BacklogRepairReport` and `LOCK_NAME`; warn on unnameable
  types.
- [x] **RV-9** (L1, L2, L3, L4) -- `create_dir_durably` takes back a chain it could not sync;
  `init` pre-scans before creating `.lock`; `repair_blob` validates indices
  (`ReplicaIndexOutOfRange`); `write_document_replicated` checks collisions before ingest.
- [x] **RV-10** (L5, L6) -- Backlog drain restores unprocessed suspects on error; a burst that
  ends on a snapshot or repair still quiesces (and, after a snapshot, drains).
- [x] **RV-11** (L17, L18) -- Hash-set intra-set collision check; replica copies are hashed
  while copying and refused on mismatch (`StagedReplicaMismatch`).

### M4 -- Tests and test support

- [x] **RV-12** (L8) -- The writer-gate test waits for the waiter to reach the gate before its
  negative check.
- [x] **RV-13** (L9) -- `dev_only` reads dependency kinds from `cargo metadata`.
- [x] **RV-14** (L10) -- Zero-sleep and repeated-interval timer tests on a paused clock.
- [x] **RV-15** (L11) -- `canopy-lock-holder` exits when its stdin closes; tests pipe it.

## Moved 2026-11-09 -- Third review fixes (GPT-6.1 Sol review)

Findings N1-N9 and the partial items of the third review. Decisions recorded in
[crates/canopy/DESIGN-NOTES.md](crates/canopy/DESIGN-NOTES.md) and
[crates/acadia/DESIGN-NOTES.md](crates/acadia/DESIGN-NOTES.md) under "Third review fixes,
2026-11-09".

- [x] **RV3-1** (N1) -- A failure that may have passed the commit point fences every handle
  (`StoreError::RecoveryRequired`); journal resets are refused until reopen. The failing
  transaction is rolled back inside `apply_transaction` when provably unprepared.
- [x] **RV3-2** (N2) -- The commit log's directory entry is synced until known durable,
  retrying after a failure.
- [x] **RV3-3** (N3) -- Namespace collision probes propagate every error but absence.
- [x] **RV3-4** (N4, M3) -- `MemFs` captures a renamed directory's durable contents at the
  rename; independent of parent sync order and of renaming over an empty directory.
- [x] **RV3-5** (N5) -- `repair_pointer` re-reads under the gate and ignores an obsolete report.
- [x] **RV3-6** (N6) -- A node and its descendants can be removed in one transaction;
  removals apply deepest first.
- [x] **RV3-7** (N7) -- Phase-boundary sweeps also run after `simulate_crash`, plus an `fsync`
  sweep.
- [x] **RV3-8** (N8, L1) -- Directories are created and synced one at a time, with unsyncable
  names remembered; quarantine directories use it.
- [x] **RV3-9** (N9, L7, L8, L13) -- Bounded EOF wait in the lock-holder test; monitor holds the
  gate per command; gate panic-release and queued-admission tests; published-crate test
  decision recorded. L10's park tests keep a liveness bound only, which does not depend on
  timing.
- [x] **RV3-10** (G1, G2 from the Grok review) -- `quiesce` reports a refused or failed reset
  instead of success or `Stopped`; MemFs keeps carried snapshots across failed removals and
  syncs.

## Moved 2026-11-09 -- Fourth review fixes (GPT-6 Astra review)

Findings A1-A7. Decisions recorded under "Fourth review fixes, 2026-11-09" in
[crates/canopy/DESIGN-NOTES.md](crates/canopy/DESIGN-NOTES.md) and
[crates/acadia/DESIGN-NOTES.md](crates/acadia/DESIGN-NOTES.md). A revert-probe confirmed each
canopy regression test fails without its fix.

- [x] **RV4-1** (A1) -- Removal targets are collision-checked before `begin`.
- [x] **RV4-2** (A2) -- Pointer and node removals sync the parent even when already absent.
- [x] **RV4-3** (A3) -- Unsynced-directory debt is persisted and discharged at open.
- [x] **RV4-4** (A4, G1 residue) -- The fenced actor runs no callback; monitor calls report the
  fence instead of `Stopped`.
- [x] **RV4-5** (A5) -- Pointers must name their own document's blob, on read and on write.
- [x] **RV4-6** (A6) -- MemFs: replaced paths start empty; directories moved out of a renamed
  directory keep their contents.
- [x] **RV4-7** (A7) -- An empty commit log is flushed until known flushed.

## Moved 2026-11-09 -- Fifth review fixes (second GPT-6 Astra review)

Findings B1-B8. Decisions recorded under "Fifth review fixes, 2026-11-09" in
[crates/canopy/DESIGN-NOTES.md](crates/canopy/DESIGN-NOTES.md) and
[crates/acadia/DESIGN-NOTES.md](crates/acadia/DESIGN-NOTES.md). A revert-probe confirmed the
regression tests for B1-B6 fail without their fixes.

- [x] **RV5-1** (B1) -- A set's removals are checked against its own writes before `begin`.
- [x] **RV5-2** (B2) -- Directory creation is recorded durably before it happens.
- [x] **RV5-3** (B3) -- On Windows, a reserved-name nonce is substituted (first character to
  U+00A9) where it forms a whole path component.
- [x] **RV5-4** (B4) -- Node removal rejects a node holding unrecognised entries.
- [x] **RV5-5** (B5) -- MemFs does not extract carried contents past a replaced path.
- [x] **RV5-6** (B6) -- The modification count is odd while a modification is in progress.
- [x] **RV5-7** (B7) -- MemFs reclaims unreachable file data.
- [x] **RV5-8** (B8) -- Superseding banners on the ECAT-12 and third-review decisions.

## Moved 2026-11-09 -- Sixth review fixes (third GPT-6 Astra review)

Findings C1-C8. Decisions recorded under "Sixth review fixes, 2026-11-09" in
[crates/canopy/DESIGN-NOTES.md](crates/canopy/DESIGN-NOTES.md) and
[crates/acadia/DESIGN-NOTES.md](crates/acadia/DESIGN-NOTES.md), with superseding banners on the
fifth-round sections. Each fix was revert-probed: its regression tests fail without it. Writing
the C2 test exposed that pointer reads failed, rather than reporting absence, for a path beneath
a document on POSIX semantics; that is fixed under RV6-2.

- [x] **RV6-1** (C1) -- A write at or above a node the same set removes is refused before
  `begin`.
- [x] **RV6-2** (C2) -- A path beneath a file is absent: stale directory intents, pointer reads
  and listings treat `NotADirectory` as `NotFound`.
- [x] **RV6-3** (C3) -- The directory-intent record is cut back after a torn append, and only
  complete, plain relative records are acted on.
- [x] **RV6-4** (C4) -- MemFs drops a rename destination's carried state before installing the
  source's.
- [x] **RV6-5** (C5) -- MemFs releases entries discarded by `refresh` and `materialize`.
- [x] **RV6-6** (C6) -- MemFs reclaims by per-inode reference count instead of a global walk.
- [x] **RV6-7** (C7) -- `BlobRef::new` validates its shard width; `write_pointer` refuses a
  target at another width.
- [x] **RV6-8** (C8) -- Blob and pointer repairs hold a modification guard.

## Moved 2026-11-10 -- Seventh review fixes (fourth GPT-6 Astra review)

Findings D1-D7. Decisions recorded under "Seventh review fixes, 2026-11-10" in
[crates/canopy/DESIGN-NOTES.md](crates/canopy/DESIGN-NOTES.md) and
[crates/acadia/DESIGN-NOTES.md](crates/acadia/DESIGN-NOTES.md). Each code fix was revert-probed:
its regression tests fail without it. The D1 probe reproduced the reported data loss -- the
document became unreadable after the crash.

- [x] **RV7-1** (D1) -- Blob collection syncs the namespace entry before deleting anything.
- [x] **RV7-2** (D2) -- A node removal at or beneath an existing document is refused before
  `begin`.
- [x] **RV7-3** (D3) -- MemFs drops live bytes only with an inode's last live name.
- [x] **RV7-4** (D4) -- MemFs syncs a directory replacing a durable file into a carried snapshot.
- [x] **RV7-5** (D5) -- `repair_blob` refuses a reference at another shard width.
- [x] **RV7-6** (D6) -- Superseding banner on the fourth-round directory-debt decision.
- [x] **RV7-7** (D7) -- `canopy-lock-holder` output goes through one reporter.

## Moved 2026-11-10 -- Eighth review fixes (fifth GPT-6 Astra review)

Findings E2-E6. Decisions recorded under "Eighth review fixes, 2026-11-10" in
[crates/acadia/DESIGN-NOTES.md](crates/acadia/DESIGN-NOTES.md). Each was revert-probed: its
regression tests fail without it, and each `MemFs` probe broke only its own test. The `E5`
probe ran on Linux, the only platform where the defect exists. `E1` (no pointer majority after
consecutive failed non-transactional writes) is not fixed here: its fix is decided from the
formal model, queued as `FM-2.1`/`FM-2.2` in [crates/canopy/CHECKLIST.md](crates/canopy/CHECKLIST.md).

- [x] **RV8-2** (E2) -- A moved directory synced over a file keeps its carried contents.
- [x] **RV8-3** (E3) -- A carried sync anchors at the deepest file and never drops a snapshot.
- [x] **RV8-4** (E4) -- Directories created inside a carried snapshot are unnamed until their
  parent syncs.
- [x] **RV8-5** (E5) -- `OsFs::read_at` refuses a directory before considering the range.
- [x] **RV8-6** (E6) -- The duplicate-target commit-log test reaches the duplicate check, with a
  distinct-target control.
