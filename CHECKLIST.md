# Checklist

## M1 -- Publishing readiness

- [ ] **PUB-1** -- Configure the `RELEASE_PLEASE_TOKEN` and `CARGO_REGISTRY_TOKEN`
  repository secrets.
- [ ] **PUB-2** -- Confirm the `canopy` and `acadia` crate names are available on
  crates.io.
  > **CROSS-COMPONENT PREREQUISITE:** publish only after `canopy`'s public write surface has
  > moved onto `Store`, so the first release does not ship `WriteMonitor` as an API it then
  > removes: component `crates/canopy` -> M2 (`WS-1`..`WS-4`). See
  > [crates/canopy/CHECKLIST.md](crates/canopy/CHECKLIST.md).

## M2 -- Source hygiene

- [ ] **SRC-1** -- Remove work-item ids from the Rust sources. The code imported with
  `canopy` and `acadia` cites the originating repository's checklist items in comments, doc
  comments and test text (`TXN-*`, `BG-*`, `CVS-*`, `LOCK-*`, `ECAT-*`, `VFS-*` and similar),
  which the repository instructions forbid ("Work-item ids never appear in source code").
  Rewrite each citation as prose saying what the code does and why, citing a design decision
  (`Dnnn`) where the reason lives in one; delete it where the surrounding prose already says
  it. Done when `git grep -E '\b[A-Z]+-M?[0-9]+(\.[0-9]+)?\b' -- '*.rs'` finds only
  decision ids, error and protocol names, and literals.
  In the same pass, rewrite comments that narrate a change rather than describe the code
  ("Comments describe the code as it is" in the repository instructions); known instances:
  [name.rs](crates/canopy/src/name.rs) (the module doc on the retired shard tier) and
  [concurrent_read.rs](crates/canopy/tests/concurrent_read.rs) (what a failed settle "used to"
  produce). A grep for *no longer*, *previously*, *used to*, *now* in comments finds
  candidates; most hits describe runtime state and stay.
