\* Copyright (c) Michael Grier.
---------------------------- MODULE PointerPublication ----------------------------
(***************************************************************************)
(* One document's non-transactional write, blob collection, crash, and     *)
(* reopen, at the granularity of the store's individual Vfs calls          *)
(* (the formal-model decision in crates/canopy/DESIGN-NOTES.md). Each     *)
(* action names the function it abstracts.                                 *)
(*                                                                         *)
(* Every step that calls the filesystem may instead fail (Fail), which     *)
(* ends the operation with an error before that step's effect. A crash     *)
(* (Crash) may happen between any two steps and reverts every live state   *)
(* to its durable state.                                                   *)
(*                                                                         *)
(* Pointer replicas hold a version or None. A pointer write renames into   *)
(* the live namespace and then fsyncs the parent, which is what makes it   *)
(* durable. Blob replicas hold sets of versions, made live by a rename and *)
(* durable by a shard fsync; removal is the reverse.                       *)
(*                                                                         *)
(* Fix selects how a write guards against STRANDED-VOTE:                   *)
(*   "none"         the write publishes over whatever the replicas hold.   *)
(*   "resolve"      before publishing, every replica is set to the live    *)
(*                  majority and synced.                                   *)
(*   "resolve-lazy" as "resolve", but only when the live replicas disagree.*)
(*   "record"       a durable per-document record names the version being *)
(*                  published; a write or a reopen that finds one rolls    *)
(*                  every replica forward to it first.                     *)
(*   "fence"        a write that fails after its first pointer rename      *)
(*                  refuses further writes until the store is reopened.    *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS
    NPtr,               \* pointer replicas (GenesisConfig pointer_replica_count)
    NBlob,              \* blob replicas (GenesisConfig blob_replica_count)
    Versions,           \* bodies a write can place
    Initial,            \* the version durable everywhere at the start
    MaxWrites,          \* bound on writes, to keep the model finite
    MaxCrashes,         \* bound on crashes
    SyncBeforeCollect,  \* collect_document_blobs syncs the entry before deleting
    Fix                 \* the STRANDED-VOTE guard under test

ASSUME Initial \in Versions
\* with_pointer_replica_count refuses an even count, so two values can never tie.
ASSUME NPtr \in Nat /\ NPtr % 2 = 1 /\ NBlob \in Nat \ {0}
ASSUME Fix \in {"none", "resolve", "resolve-lazy", "record", "fence"}

None == "none"
NoMajority == "no-majority"
Values == Versions \cup {None}
Ptr == 1..NPtr
Blob == 1..NBlob

VARIABLES
    ptrLive,     \* [Ptr -> Values]: the live namespace entry per replica
    ptrDur,      \* [Ptr -> Values]: what a crash would leave
    blobLive,    \* [Blob -> SUBSET Versions]: blob files present per replica
    blobDur,     \* [Blob -> SUBSET Versions]
    mode,        \* "up" | "down" (no open handle) | "recovering" (Store::open)
    task,        \* "none" | "write" | "recover"
    phase,       \* the next step of task
    ver,         \* the version the current write places
    idx,         \* the replica the current step is at
    keep,        \* the keep-set collection read from the namespace
    journalled,  \* an ended transaction for this document is still in the log
    target,      \* "resolve": the value every replica is being set to
    record,      \* "record": the durable publication record, a version or None
    fenced,      \* "fence": writes refused until reopen (process memory)
    writes,
    crashes

vars == <<ptrLive, ptrDur, blobLive, blobDur, mode, task, phase, ver, idx, keep,
          journalled, target, record, fenced, writes, crashes>>

-----------------------------------------------------------------------------
(* The pointer vote (namespace.rs read_pointer, pointer_majority): a value *)
(* held by a strict majority of replicas, absence included.                *)
Holding(f, v) == Cardinality({r \in Ptr : f[r] = v})
Majority(f) ==
    IF \E v \in Values : 2 * Holding(f, v) > NPtr
    THEN CHOOSE v \in Values : 2 * Holding(f, v) > NPtr
    ELSE NoMajority
Unanimous(f) == \A r \in Ptr : f[r] = f[1]

TypeOK ==
    /\ ptrLive \in [Ptr -> Values]
    /\ ptrDur \in [Ptr -> Values]
    /\ blobLive \in [Blob -> SUBSET Versions]
    /\ blobDur \in [Blob -> SUBSET Versions]
    /\ mode \in {"up", "down", "recovering"}
    /\ task \in {"none", "write", "recover"}
    /\ ver \in Versions
    /\ idx \in Ptr \cup Blob
    /\ keep \in SUBSET Versions
    /\ journalled \in BOOLEAN
    /\ target \in Values
    /\ record \in Values
    /\ fenced \in BOOLEAN

Init ==
    /\ ptrLive = [r \in Ptr |-> Initial]
    /\ ptrDur = ptrLive
    /\ blobLive = [b \in Blob |-> {Initial}]
    /\ blobDur = blobLive
    /\ mode = "up"
    /\ task = "none"
    /\ phase = "none"
    /\ ver = Initial
    /\ idx = 1
    /\ keep = {}
    \* The write that placed Initial was a transaction whose ended record is
    \* still in the log, so every reopen re-runs its collection (store.rs recover).
    /\ journalled = TRUE
    /\ target = Initial
    /\ record = None
    /\ fenced = FALSE
    /\ writes = 0
    /\ crashes = 0

-----------------------------------------------------------------------------
(* Ending an operation. A failed recovery fails Store::open. A failed      *)
(* write leaves the store up; under "fence" it also refuses further       *)
(* writes once any pointer replica has been renamed.                       *)
Finish ==
    /\ task' = "none"
    /\ phase' = "none"
    /\ mode' = IF task = "recover" THEN "up" ELSE mode

Publishing == phase = "ptrSync" \/ (phase = "ptrRename" /\ idx > 1)

Fail ==
    /\ task # "none"
    /\ task' = "none"
    /\ phase' = "none"
    /\ mode' = IF task = "recover" THEN "down" ELSE mode
    /\ fenced' = (fenced \/ (Fix = "fence" /\ task = "write" /\ Publishing))
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, ver, idx, keep,
                   journalled, target, record, writes, crashes>>

-----------------------------------------------------------------------------
(* document.rs write_document_replicated: guard, ingest, publish, collect. *)
FirstWritePhase ==
    CASE Fix = "resolve"                              -> "resStart"
      [] Fix = "resolve-lazy" /\ ~Unanimous(ptrLive)  -> "resStart"
      [] Fix = "record" /\ record # None              -> "rfRename"
      [] OTHER                                        -> "ingestRename"

StartWrite(v) ==
    /\ mode = "up" /\ task = "none" /\ writes < MaxWrites /\ ~fenced
    /\ task' = "write" /\ phase' = FirstWritePhase /\ ver' = v /\ idx' = 1
    /\ writes' = writes + 1
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, mode, keep, journalled,
                   target, record, fenced, crashes>>

\* "resolve": read the vote; with no majority there is nothing to resolve to.
ResStart ==
    /\ task = "write" /\ phase = "resStart"
    /\ LET m == Majority(ptrLive)
       IN  IF m = NoMajority
           THEN Finish /\ UNCHANGED target
           ELSE target' = m /\ phase' = "resRename" /\ UNCHANGED <<task, mode>>
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, ver, idx, keep, journalled,
                   record, fenced, writes, crashes>>

\* Set replica idx to the target (a rename, or a removal for None) ...
ResRename ==
    /\ task = "write" /\ phase = "resRename"
    /\ ptrLive' = [ptrLive EXCEPT ![idx] = target]
    /\ phase' = "resSync"
    /\ UNCHANGED <<ptrDur, blobLive, blobDur, mode, task, ver, idx, keep, journalled,
                   target, record, fenced, writes, crashes>>

\* ... and sync its parent, so the durable replicas agree before publishing.
ResSync ==
    /\ task = "write" /\ phase = "resSync"
    /\ ptrDur' = [ptrDur EXCEPT ![idx] = ptrLive[idx]]
    /\ IF idx < NPtr
       THEN idx' = idx + 1 /\ phase' = "resRename"
       ELSE idx' = 1 /\ phase' = "ingestRename"
    /\ UNCHANGED <<ptrLive, blobLive, blobDur, mode, task, ver, keep, journalled,
                   target, record, fenced, writes, crashes>>

\* blob.rs ingest_blob phase 3: rename the staged replica into its shard ...
IngestRename ==
    /\ task = "write" /\ phase = "ingestRename"
    /\ blobLive' = [blobLive EXCEPT ![idx] = @ \cup {ver}]
    /\ phase' = "ingestSync"
    /\ UNCHANGED <<ptrLive, ptrDur, blobDur, mode, task, ver, idx, keep, journalled,
                   target, record, fenced, writes, crashes>>

\* ... then fsync the shard, replica by replica.
IngestSync ==
    /\ task = "write" /\ phase = "ingestSync"
    /\ blobDur' = [blobDur EXCEPT ![idx] = blobLive[idx]]
    /\ IF idx < NBlob
       THEN idx' = idx + 1 /\ phase' = "ingestRename"
       ELSE idx' = 1 /\ phase' = IF Fix = "record" THEN "recWrite" ELSE "ptrRename"
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, mode, task, ver, keep, journalled,
                   target, record, fenced, writes, crashes>>

\* "record": durably name the version about to be published (append + fsync).
RecWrite ==
    /\ task = "write" /\ phase = "recWrite"
    /\ record' = ver
    /\ phase' = "ptrRename"
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, mode, task, ver, idx, keep,
                   journalled, target, fenced, writes, crashes>>

\* namespace.rs write_pointer: every replica staged first (a failure there is
\* Fail at the first rename), then per replica rename into place ...
PtrRename ==
    /\ task = "write" /\ phase = "ptrRename"
    /\ ptrLive' = [ptrLive EXCEPT ![idx] = ver]
    /\ phase' = "ptrSync"
    /\ UNCHANGED <<ptrDur, blobLive, blobDur, mode, task, ver, idx, keep, journalled,
                   target, record, fenced, writes, crashes>>

\* ... and fsync the entry's parent.
PtrSync ==
    /\ task = "write" /\ phase = "ptrSync"
    /\ ptrDur' = [ptrDur EXCEPT ![idx] = ptrLive[idx]]
    /\ IF idx < NPtr
       THEN idx' = idx + 1 /\ phase' = "ptrRename"
       ELSE idx' = 1 /\ phase' = IF Fix = "record" THEN "recClear" ELSE "collRead"
    /\ UNCHANGED <<ptrLive, blobLive, blobDur, mode, task, ver, keep, journalled,
                   target, record, fenced, writes, crashes>>

\* "record": the publication is complete and durable, so the record goes
\* (truncate + fsync).
RecClear ==
    /\ task = "write" /\ phase = "recClear"
    /\ record' = None
    /\ phase' = "collRead"
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, mode, task, ver, idx, keep,
                   journalled, target, fenced, writes, crashes>>

-----------------------------------------------------------------------------
(* "record": roll every replica forward to the recorded version, then      *)
(* clear the record. Run by a write before its own work and by a reopen    *)
(* before recovery.                                                        *)
RfRename ==
    /\ task # "none" /\ phase = "rfRename"
    /\ ptrLive' = [ptrLive EXCEPT ![idx] = record]
    /\ phase' = "rfSync"
    /\ UNCHANGED <<ptrDur, blobLive, blobDur, mode, task, ver, idx, keep, journalled,
                   target, record, fenced, writes, crashes>>

RfSync ==
    /\ task # "none" /\ phase = "rfSync"
    /\ ptrDur' = [ptrDur EXCEPT ![idx] = ptrLive[idx]]
    /\ IF idx < NPtr
       THEN idx' = idx + 1 /\ phase' = "rfRename"
       ELSE idx' = 1 /\ phase' = "rfClear"
    /\ UNCHANGED <<ptrLive, blobLive, blobDur, mode, task, ver, keep, journalled,
                   target, record, fenced, writes, crashes>>

RfClear ==
    /\ task # "none" /\ phase = "rfClear"
    /\ record' = None
    /\ CASE task = "write" -> phase' = "ingestRename" /\ UNCHANGED <<task, mode>>
         [] journalled     -> phase' = "collRead" /\ UNCHANGED <<task, mode>>
         [] OTHER          -> Finish
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, ver, idx, keep, journalled,
                   target, fenced, writes, crashes>>

-----------------------------------------------------------------------------
(* blob.rs collect_document_blobs, shared by a write and by recovery.      *)

\* Read the keep-set from the live namespace. Every replica absent keeps
\* nothing; a majority keeps its version; anything else skips collection.
CollectFrom(k) ==
    /\ keep' = k
    /\ idx' = 1
    /\ phase' = IF SyncBeforeCollect THEN "collSync" ELSE "collRemove"
    /\ UNCHANGED <<task, mode>>

CollRead ==
    /\ task # "none"
    /\ phase = "collRead"
    /\ LET m == Majority(ptrLive)
       IN  CASE \A r \in Ptr : ptrLive[r] = None -> CollectFrom({})
             [] m \in Versions                   -> CollectFrom({m})
             [] OTHER                            -> Finish /\ UNCHANGED <<keep, idx>>
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, ver, journalled, target, record,
                   fenced, writes, crashes>>

\* namespace.rs sync_pointer_entry: fsync each replica's entry parent.
CollSync ==
    /\ task # "none" /\ phase = "collSync"
    /\ ptrDur' = [ptrDur EXCEPT ![idx] = ptrLive[idx]]
    /\ IF idx < NPtr
       THEN idx' = idx + 1 /\ phase' = "collSync"
       ELSE idx' = 1 /\ phase' = "collRemove"
    /\ UNCHANGED <<ptrLive, blobLive, blobDur, mode, task, ver, keep, journalled,
                   target, record, fenced, writes, crashes>>

\* blob.rs retain_blobs: per replica, remove every blob not kept, and fsync
\* the shard only if something was removed.
NextBlobOrFinish ==
    IF idx < NBlob
    THEN idx' = idx + 1 /\ phase' = "collRemove" /\ UNCHANGED <<task, mode>>
    ELSE Finish /\ UNCHANGED idx

CollRemove ==
    /\ task # "none" /\ phase = "collRemove"
    /\ IF blobLive[idx] \subseteq keep
       THEN NextBlobOrFinish /\ UNCHANGED blobLive
       ELSE /\ blobLive' = [blobLive EXCEPT ![idx] = @ \cap keep]
            /\ phase' = "collFsync"
            /\ UNCHANGED <<task, mode, idx>>
    /\ UNCHANGED <<ptrLive, ptrDur, blobDur, ver, keep, journalled, target, record,
                   fenced, writes, crashes>>

CollFsync ==
    /\ task # "none" /\ phase = "collFsync"
    /\ blobDur' = [blobDur EXCEPT ![idx] = blobLive[idx]]
    /\ NextBlobOrFinish
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, ver, keep, journalled, target, record,
                   fenced, writes, crashes>>

-----------------------------------------------------------------------------
(* Process lifecycle.                                                      *)

\* EXTERNAL: power loss. Everything not durable is gone, and so is the process.
Crash ==
    /\ crashes < MaxCrashes
    /\ ptrLive' = ptrDur
    /\ blobLive' = blobDur
    /\ mode' = "down" /\ task' = "none" /\ phase' = "none"
    /\ fenced' = FALSE
    /\ crashes' = crashes + 1
    /\ UNCHANGED <<ptrDur, blobDur, ver, idx, keep, journalled, target, record, writes>>

\* The last handle is dropped without a crash.
Close ==
    /\ mode = "up" /\ task = "none"
    /\ mode' = "down"
    /\ fenced' = FALSE
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, task, phase, ver, idx, keep,
                   journalled, target, record, writes, crashes>>

\* store.rs Store::open -> recover: a "record" roll-forward first, then
\* collection is re-run for every ended transaction still in the log.
Open ==
    /\ mode = "down"
    /\ idx' = 1
    /\ CASE Fix = "record" /\ record # None ->
              mode' = "recovering" /\ task' = "recover" /\ phase' = "rfRename"
         [] journalled ->
              mode' = "recovering" /\ task' = "recover" /\ phase' = "collRead"
         [] OTHER ->
              mode' = "up" /\ UNCHANGED <<task, phase>>
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, ver, keep, journalled, target,
                   record, fenced, writes, crashes>>

\* The quiescence reset empties the log.
Quiesce ==
    /\ mode = "up" /\ task = "none" /\ journalled
    /\ journalled' = FALSE
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, mode, task, phase, ver, idx,
                   keep, target, record, fenced, writes, crashes>>

Next ==
    \/ \E v \in Versions : StartWrite(v)
    \/ ResStart \/ ResRename \/ ResSync
    \/ IngestRename \/ IngestSync \/ RecWrite \/ PtrRename \/ PtrSync \/ RecClear
    \/ RfRename \/ RfSync \/ RfClear
    \/ CollRead \/ CollSync \/ CollRemove \/ CollFsync
    \/ Fail \/ Crash \/ Close \/ Open \/ Quiesce

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
(* Properties.                                                             *)

\* DANGLING-DURABLE: whatever a crash would leave, a pointer majority names a
\* blob that survives on some replica.
DurablePointerHasBlob ==
    Majority(ptrDur) \in Versions => \E b \in Blob : Majority(ptrDur) \in blobDur[b]

\* The same for the live state a reader sees.
LivePointerHasBlob ==
    Majority(ptrLive) \in Versions => \E b \in Blob : Majority(ptrLive) \in blobLive[b]

\* STRANDED-VOTE: with no operation in progress, the namespace has a majority --
\* a version or absence -- so a read is never told to wait for a writer that
\* does not exist.
IdleImpliesSettled ==
    (mode = "up" /\ task = "none") => Majority(ptrLive) # NoMajority
=============================================================================
