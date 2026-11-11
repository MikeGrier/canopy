# COMPLETED CHECKLIST -- acadia

Append-only archive of completed items from [CHECKLIST.md](CHECKLIST.md).

## Moved 2026-11-10 -- MemFs directory sync cost

### <a id="mf-1"></a>MF-1 -- Make a `MemFs` directory sync cost what changed rather than the directory's size. *(completed 2026-11-10 19:36:32 -05:00)*

A directory sync reconciled every entry, so a store syncing a partition root after each new
shard was quadratic in the shard count; profiling the content-store scale tests put most of
their time in `MemFs::fsync`. Each directory now records the names changed since its last
sync and a sync reconciles only those, or everything after a wholesale replacement. The
design and its oracle are recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md) "MemFs directory
sync costs what changed". A revert-probe that dropped the record a rename makes in its
destination parent failed the differential test against the full-reconcile oracle.

### <a id="mf-2"></a>MF-2 -- Add `MemFs::fork`, an independent copy of a filesystem's whole state, so a fixture is built once. *(completed 2026-11-10)*

Tests that need the same expensive populated store each built their own; a fork copies the
live and durable trees and every piece of pending bookkeeping, so a crash of the fork behaves
exactly as a crash of the original would have, and the two never affect each other. Locks are
not copied, because they belong to their holders. Recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md)
"MemFs fork".
