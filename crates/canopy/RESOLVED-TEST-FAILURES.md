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
