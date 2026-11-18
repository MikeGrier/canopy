# Unresolved Test Failures

## Observed 2026-11-16 13:14:10 -05:00 -- Concurrent creation loser outcome

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
