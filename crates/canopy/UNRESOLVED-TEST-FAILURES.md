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