# Resolved Test Failures

## Resolved 2026-11-17 22:14:25 -05:00 -- Concurrent creation loser outcome

Store creation is idempotent ([DESIGN-NOTES.md](DESIGN-NOTES.md) "Store creation is idempotent"),
so a creator that loses the race no longer has an outcome that depends on when it looks: it waits
on the creation exclusion, finds the store of its identity, and succeeds with it. The test that
asserted a refusal was removed; its replacement,
`tests::creation::identical_concurrent_creators_make_one_store_and_none_loses_anothers_write` in
[creation.rs](src/tests/creation.rs), requires every one of eight racing creators to succeed and
every creator's write to survive. Looping its test binary 100 times, at 12 rounds a run, failed
none; a revert-probe that made the identical-store path refuse failed it at once. The original
observation follows, unchanged.

### Observed 2026-11-16 13:14:10 -05:00 -- Concurrent creation loser outcome

During `cargo test -p canopy --lib`,
`tests::lock::identical_concurrent_creators_never_replace_a_completed_store` failed at
the assertion that the losing creator returns `StoreError::AlreadyInitialized` in
[lock.rs](src/tests/lock.rs#L381). The preceding single-success assertion did not fail.
The test passed when rerun in isolation with `--exact`; this does not establish a fix.

Neither the test nor [genesis.rs](src/store/genesis.rs) was changed in the worktree.
The failure was observed while validating the publication replay work, but its cause is
unestablished. The assertion does not report the losing error, so the recorded output
does not identify which alternative outcome occurred. Full output is in the local
scratch captures [canopy-library-m11-6.ndjson](../../.scratch/canopy-library-m11-6.ndjson)
and [concurrent-creation-isolated.ndjson](../../.scratch/concurrent-creation-isolated.ndjson).

Reproduce with the library suite and the exact test above; retain the losing outcome
when investigating. Do not mark this observation resolved merely because a later run passes.
- **Update, 2026-11-17 19:32:09 -05:00 -- the losing outcome is identified.** With the assertion
  extended to report the loser's error, the failing rounds show
  `NotEmpty { root: "store", entry: "files" }`. `MemFs` lists a directory in sorted order, so
  `.store` would precede `files`; the listing therefore had no marker, and the winner was still
  building its trees. The winner holds writer exclusion until the marker is published, so the
  refusal comes from `init_through`'s first occupancy check in
  [genesis.rs](src/store/genesis.rs), which runs before exclusion. The test requires
  `AlreadyInitialized`, a stronger outcome than
  [DESIGN-NOTES.md](DESIGN-NOTES.md) "Writer exclusion belongs to an operation" promises (it covers a creator
  that finds the completed marker). Looping the library test binary reproduced it in 3 of 25 runs
  on acadia at `c62f9ca` and in 1 of 25 with acadia's M18 changes applied, so it predates them.
  Settling which side is wrong is queued as `G-2` in [CHECKLIST.md](CHECKLIST.md).

- **Update, 2026-11-17 20:51:13 -05:00 -- the behavior the test asserts is being redefined.** The
  engineer decided that store creation is idempotent ([DESIGN-NOTES.md](DESIGN-NOTES.md) "Store
  creation is idempotent"): an identical creator waits for the creation it shares and succeeds with
  the store, so the outcome no longer depends on when it looks. The item queued above as `G-2`, an
  id a completed native-wait item already carries, is withdrawn; the work is
  milestone M15 in [CHECKLIST.md](CHECKLIST.md), whose last item replaces this test and resolves
  this entry.

## Resolved 2026-11-17 22:33:07 -05:00 -- A marker published through a hard link cannot be replaced at once on Windows

On Windows acadia's `publish_new` now moves its staged file into place without replacing, rather
than hard-linking it and removing the staging name (acadia's
[DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md) "A publication is a move on Windows"). The
cross-process creation tests passed 20 looped runs, and acadia's test that publishes and at once
replaces a file fails with the hard link and passes with the move. The original observation
follows, unchanged.

### Observed 2026-11-17 22:40:00 -05:00 -- A marker published through a hard link cannot be replaced at once on Windows

canopy-lock-holder's `creation::` tests in `shared_count` fail on Windows in about half of runs:
a creator's agent-version stamp, which replaces `.store` by renaming `.store.tmp` over it, fails
with `Access is denied. (os error 5)`. It reproduces in one process with no store open, and with
plain `std::fs` and no canopy or acadia code: a file made by writing a staging file,
`hard_link`ing it to `.store` and removing the staging name could not be renamed over for up to
about 1.3 s in about one round in eight, and retrying cleared it every time. The same sequence with
`.store` made by a rename instead never failed. Every open in acadia's `OsFs` goes through
`std::fs`, whose default share mode includes `FILE_SHARE_DELETE`, and nothing in the process held
the file, so the blocking handle is outside the process; Defender real-time protection was on.
The marker has been published with `publish_new` (staging, hard link, remove staging) since store
creation became idempotent; before that it was published by a rename and the stamp did not fail.

## Resolved 2026-11-20 15:29:27 -05:00 -- Regenerated SharedPublication transaction replay departs early

Withdrawn with the work that produced it: the engineer dropped the saved recovery adaptation and
the corpus regenerated from its model, and origin's committed corpus replays in full against
origin's code. Whether the departures exposed a limitation of the replay bridge or a defect in the
adaptation was not established. The original observation follows, unchanged.

### Observed 2026-11-19 -- Regenerated SharedPublication transaction replay departs early

**Revision:** adapted recovery changes in the isolated `.scratch/recovery-eval-4`
worktree based on origin `6b42627`. The trace corpus was regenerated from the adapted
SharedPublication model using the repository tools and published TLC baseline.

**Observed:** the SharedPublication replay module passed 52 of 58 tests. Six regenerated
simulation replays failed. One healthy-publication seed reported that the ownership record
was absent where the model expected it. Five transaction or shared-transaction seeds showed
the store projection ahead of the model at a commit-log boundary; examples include the
store reporting `ended` while the model still reports `begun`, and a transaction effect
already applied while the model still reports a prepare. The failures occur at replay
checkpoint assertions, not at TLC checks or the focused recovery production tests.

**Expected:** each regenerated trace replays step-for-step against the Store bridge or
records a deliberate, classified departure. The failure concentration suggests the
replay checkpoint cannot distinguish repeated commit-log or rollback operations in a
single Store call. That hypothesis is not yet proven, and the ownership-record mismatch
must be isolated separately before attributing all six to one cause.

**Scope:** isolated adapted code, regenerated SharedPublication corpus, Microsoft JDK 25,
and the published TLC 1.8.0 jar. The focused recovery, publication, upgrade, process,
left-claim witness, and T27 replay checks did not fail. No production or bridge fix was
applied in this observation. Follow-up is queued as `RECOVERY-REPLAY-BRIDGE` in
[CHECKLIST-recovery-sync-evaluation.md](CHECKLIST-recovery-sync-evaluation.md); the full
context and commands are in
[recovery-eval-4-results.md](../../.scratch/recovery-eval-4-results.md).

- **Update, 2026-11-20 -- the failures are confined to the locally regenerated corpus.**
  At origin `6b42627`, unmodified, the SharedPublication replay module passes in full
  against origin's committed corpus. Re-exporting origin's unmodified model with the current
  published TLC 1.8.0 jar reproduces that committed corpus byte for byte, so the toolchain
  change does not alter the traces. After the conflict resolution the branch carries the
  adapted corpus and fails the same six seeds: `transaction_seed_1` through `_3`,
  `shared_transaction_seed_2` and `_3`, and `healthy_publication_seed_3`. Origin's newer
  commits through `2be34aa` change neither the spec nor the corpus. It remains open whether
  the adapted model's new walks expose a pre-existing bridge limitation or a defect in the
  adapted code.

## Resolved 2026-11-20 15:40:17 -05:00 -- Recovery completion leaves an intervening writer's stale claim

A recovery run now rechecks the claims directory when it reaches a later phase after an intervening
holder. A surviving claim rewinds the run to the claims phase and adds the repeated unit to progress
accounting; an intervening holder with no claim does not rewind. The original observation follows,
unchanged.

### Observed 2026-11-19 -- Recovery completion leaves an intervening writer's stale claim

**Revision:** origin `6b42627`, reproduced in an isolated checkout with production code
unchanged. The regression test is
`recovery_completion_releases_a_claim_left_by_an_intervening_document_writer`, added to the
isolated copy of [recovery_run.rs](src/tests/recovery_run.rs).

**Reproduction:** start recovery of a healthy store and pause at `RecoveryPhase::Completion`,
after its claims phase. Open a second writer using another process view of the same `MemFs`.
Invoke the production `Store::write_document_replicated` API, observing the claims-directory
sync only when that operation's actual nonce claim exists. At that boundary call `abandon`
on the second process view. The document write then fails; the process view rejects its
cleanup I/O as well, leaving its claim without opening a modification. Verify that the
claim exists, the modification count is unchanged, and the document was not written.
Resume the paused recovery: it reports finished while the claim remains.

**Expected:** recovery accounts for the claim left by the intervening holder before reporting
completion. **Observed assertion:** `recovery reported completion while the intervening
writer's claim remained`. The same interruption while recovery is at publication or claims
releases the claim. A fresh recovery also releases the claim the stale completed run left.
These controls distinguish stale recovery progress from an active or unremovable claim.

**Scope:** deterministic unit-level reproduction through a real write and acadia's simulated
process-termination semantics. No operating-system process was killed and no document
corruption was established. The surrounding recovery module had no other failure in this
run. The saved patch has not been validated by this reproduction, and no fix was applied.
The fix and the production-path regression are queued as `M11-10` in
[CHECKLIST.md](CHECKLIST.md).

- **Update, 2026-11-20 -- `origin/main` cannot reach this state.** On `origin/main`
  (`ed0dadb`, an ancestor of this branch), `Store::open` takes the root's exclusive writer
  lock and holds it for the store's life (`D116`), recovers within `open`, and then releases
  every claim, so no holder can come between recovery's steps. The window opened on
  `mikegrier/coord`: `6065221` made recovery a stepped run with a claims phase and removed the
  open-time release of every claim, and `867d2ff` let several processes write under exclusion
  taken per operation. The defect is in work not yet merged to main.
