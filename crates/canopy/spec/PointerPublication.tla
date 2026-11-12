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
(*                                                                         *)
(* RecordDetail separates the "record" guard's record into what a process  *)
(* sees and what a crash leaves: writing it is an append followed by an    *)
(* fsync, clearing it a truncation followed by an fsync, and either can    *)
(* fail. Without it each is one step that cannot fail part-way. A crash    *)
(* loses what was not synced, and a torn append, once detected, is a lost  *)
(* one.                                                                    *)
(*                                                                         *)
(* Control selects a fault to confirm the properties detect it:            *)
(*   "none"                  the protocol.                                 *)
(*   "record-not-synced"     the record is not fsynced before the pointer  *)
(*                           renames begin (needs RecordDetail).           *)
(*   "record-cleared-early"  the record is cleared before the pointer      *)
(*                           renames begin.                                *)
(*   "settle-without-sync"   a settle rolls forward from a record that     *)
(*                           is in the file and not yet durable.           *)
(*   "settle-looks-only-at-the-file"                                       *)
(*                           a settle fsyncs the record only when the file *)
(*                           shows one (needs RecordDetail).               *)
(*   "logged-write-skips-the-guard"                                        *)
(*                           a logged write does not run the guard (needs  *)
(*                           LoggedWrites).                                *)
(*   "write-skips-the-guard" a write does not run the guard.               *)
(*   "check-skips-the-settle", "collect-skips-the-settle",                 *)
(*   "repair-skips-the-settle", "scrub-skips-the-settle"                   *)
(*                           that operation does not settle a pending      *)
(*                           record first (needs Maintenance).             *)
(*                                                                         *)
(* Maintenance adds the operations that read or repair a document without  *)
(* publishing to it -- a precondition check, a collection pass, a repair   *)
(* and a scrub -- and the loss of a pointer replica, which a repair fills  *)
(* with the vote's version. Under the "record" guard each begins by        *)
(* settling a pending record.                                              *)
(*                                                                         *)
(* LoggedWrites adds the write a transaction makes: its prepare record in  *)
(* the log, its pointer swaps, its end record and the collection that      *)
(* follows, and a recovery that rolls a prepared transaction forward. It   *)
(* runs the same guard as an unlogged write, and writes no record of its   *)
(* own.                                                                    *)
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
    Fix,                \* the STRANDED-VOTE guard under test
    RecordDetail,       \* "record": its live and durable state are separate
    LoggedWrites,       \* a transaction can write the document too
    Maintenance,        \* checks, collection, repair and scrub, and a lost pointer replica
    Control             \* a fault to confirm the properties detect it

ASSUME Initial \in Versions
\* with_pointer_replica_count refuses an even count, so two values can never tie.
ASSUME NPtr \in Nat /\ NPtr % 2 = 1 /\ NBlob \in Nat \ {0}
ASSUME Fix \in {"none", "resolve", "resolve-lazy", "record", "fence"}
ASSUME RecordDetail \in BOOLEAN
ASSUME LoggedWrites \in BOOLEAN
ASSUME Control \in {"none", "record-not-synced", "record-cleared-early", "settle-without-sync",
                    "settle-looks-only-at-the-file", "logged-write-skips-the-guard",
                    "write-skips-the-guard", "check-skips-the-settle", "collect-skips-the-settle",
                    "repair-skips-the-settle", "scrub-skips-the-settle"}
ASSUME Control \in {"record-not-synced", "settle-without-sync",
                    "settle-looks-only-at-the-file"} => RecordDetail
ASSUME Control = "logged-write-skips-the-guard" => LoggedWrites
ASSUME Maintenance \in BOOLEAN
ASSUME Control \in {"check-skips-the-settle", "collect-skips-the-settle",
                    "repair-skips-the-settle", "scrub-skips-the-settle"} => Maintenance

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
    txPrep,      \* "LoggedWrites": the version a prepared transaction names, or None
    target,      \* "resolve": the value every replica is being set to
    record,      \* "record": the durable publication record, a version or None
    fenced,      \* "fence": writes refused until reopen (process memory)
    writes,
    crashes

vars == <<ptrLive, ptrDur, blobLive, blobDur, mode, task, phase, ver, idx, keep,
          journalled, txPrep, target, record, fenced, writes, crashes>>

-----------------------------------------------------------------------------
(* The pointer vote (namespace.rs read_pointer, pointer_majority): a value *)
(* held by a strict majority of replicas, absence included.                *)
Holding(f, v) == Cardinality({r \in Ptr : f[r] = v})
Majority(f) ==
    IF \E v \in Values : 2 * Holding(f, v) > NPtr
    THEN CHOOSE v \in Values : 2 * Holding(f, v) > NPtr
    ELSE NoMajority
Unanimous(f) == \A r \in Ptr : f[r] = f[1]

\* The record the guard keeps: what a process reads in its file, and what a crash leaves.
NoRecord == [live |-> None, dur |-> None]

\* Where a write goes once its record is in place.
AfterRecord == IF Control = "record-cleared-early" THEN "recClearEarly" ELSE "ptrRename"

\* Where a settle of a pending record begins: by making the record durable, since a record
\* that is in the file and not yet durable is lost by a crash, and rolling forward from it
\* would make pointer replicas durable with nothing left to finish them.
SettleFirstPhase ==
    IF RecordDetail /\ Control # "settle-without-sync" THEN "rfRecSync" ELSE "rfRename"

\* A write of either kind: an unlogged one, or one a transaction makes.
Writing == task \in {"write", "txwrite"}

\* Whether an operation begins by settling the record. With RecordDetail it always syncs the
\* file first: a failed clear leaves the file empty and the durable record stale, which a
\* look at the file cannot show.
Settles ==
    /\ Fix = "record"
    /\ \/ record.live # None
       \/ RecordDetail /\ Control \notin {"settle-looks-only-at-the-file", "settle-without-sync"}

\* A repair or a scrub: both fill a missing pointer replica with the vote's version.
Maintaining == task \in {"repair", "scrub"}

\* Where a recovery begins.
RecoverPhase ==
    IF Settles THEN SettleFirstPhase
    ELSE IF txPrep # None THEN "txRfRename" ELSE "collRead"

\* Audience: internal.
TypeOK ==
    /\ ptrLive \in [Ptr -> Values]
    /\ ptrDur \in [Ptr -> Values]
    /\ blobLive \in [Blob -> SUBSET Versions]
    /\ blobDur \in [Blob -> SUBSET Versions]
    /\ mode \in {"up", "down", "recovering"}
    /\ task \in {"none", "write", "txwrite", "recover", "check", "collect", "repair", "scrub"}
    /\ ver \in Versions
    /\ idx \in Ptr \cup Blob
    /\ keep \in SUBSET Versions
    /\ journalled \in BOOLEAN
    /\ txPrep \in Values
    /\ target \in Values
    /\ record \in [live : Values, dur : Values]
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
    /\ txPrep = None
    /\ target = Initial
    /\ record = NoRecord
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
    /\ mode' = IF mode = "recovering" THEN "up" ELSE mode

Publishing == phase = "ptrSync" \/ (phase = "ptrRename" /\ idx > 1)

Fail ==
    /\ task # "none"
    /\ task' = "none"
    /\ phase' = "none"
    /\ mode' = IF mode = "recovering" THEN "down" ELSE mode
    /\ fenced' = (fenced \/ (Fix = "fence" /\ task = "write" /\ Publishing))
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, ver, idx, keep,
                   journalled, txPrep, target, record, writes, crashes>>

-----------------------------------------------------------------------------
(* document.rs write_document_replicated: guard, ingest, publish, collect. *)
FirstWritePhase ==
    CASE Fix = "resolve"                              -> "resStart"
      [] Fix = "resolve-lazy" /\ ~Unanimous(ptrLive)  -> "resStart"
      [] Settles                                      -> SettleFirstPhase
      [] OTHER                                        -> "ingestRename"

StartWrite(v) ==
    /\ mode = "up" /\ task = "none" /\ writes < MaxWrites /\ ~fenced /\ txPrep = None
    /\ task' = "write" /\ ver' = v /\ idx' = 1
    /\ phase' = IF Control = "write-skips-the-guard" THEN "ingestRename" ELSE FirstWritePhase
    /\ writes' = writes + 1
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, mode, keep, journalled, txPrep,
                   target, record, fenced, crashes>>

\* A transaction writes the document. It runs the guard first, as an unlogged write does.
StartTx(v) ==
    /\ LoggedWrites
    /\ mode = "up" /\ task = "none" /\ writes < MaxWrites /\ ~fenced /\ txPrep = None
    /\ task' = "txwrite"
    /\ phase' = IF Control = "logged-write-skips-the-guard" THEN "ingestRename" ELSE FirstWritePhase
    /\ ver' = v /\ idx' = 1
    /\ writes' = writes + 1
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, mode, keep, journalled, txPrep,
                   target, record, fenced, crashes>>

\* Whether the control omits the settle of operation k.
SkipsSettle(k) ==
    CASE k = "check"   -> Control = "check-skips-the-settle"
      [] k = "collect" -> Control = "collect-skips-the-settle"
      [] k = "repair"  -> Control = "repair-skips-the-settle"
      [] OTHER         -> Control = "scrub-skips-the-settle"

\* A check, a collection pass, a repair or a scrub of the document. Under the "record" guard it
\* settles a pending record first.
StartOp(k) ==
    /\ Maintenance
    /\ mode = "up" /\ task = "none" /\ txPrep = None
    /\ task' = k /\ idx' = 1
    /\ phase' = IF Settles /\ ~SkipsSettle(k)
                THEN SettleFirstPhase
                ELSE CASE k = "check"   -> "chkRead"
                       [] k = "collect" -> "collRead"
                       [] OTHER         -> "repRead"
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, mode, ver, keep, journalled, txPrep,
                   target, record, fenced, writes, crashes>>

\* "resolve": read the vote; with no majority there is nothing to resolve to.
ResStart ==
    /\ Writing /\ phase = "resStart"
    /\ LET m == Majority(ptrLive)
       IN  IF m = NoMajority
           THEN Finish /\ UNCHANGED target
           ELSE target' = m /\ phase' = "resRename" /\ UNCHANGED <<task, mode>>
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, ver, idx, keep, journalled, txPrep,
                   record, fenced, writes, crashes>>

\* Set replica idx to the target (a rename, or a removal for None) ...
ResRename ==
    /\ Writing /\ phase = "resRename"
    /\ ptrLive' = [ptrLive EXCEPT ![idx] = target]
    /\ phase' = "resSync"
    /\ UNCHANGED <<ptrDur, blobLive, blobDur, mode, task, ver, idx, keep, journalled, txPrep,
                   target, record, fenced, writes, crashes>>

\* ... and sync its parent, so the durable replicas agree before publishing.
ResSync ==
    /\ Writing /\ phase = "resSync"
    /\ ptrDur' = [ptrDur EXCEPT ![idx] = ptrLive[idx]]
    /\ IF idx < NPtr
       THEN idx' = idx + 1 /\ phase' = "resRename"
       ELSE idx' = 1 /\ phase' = "ingestRename"
    /\ UNCHANGED <<ptrLive, blobLive, blobDur, mode, task, ver, keep, journalled, txPrep,
                   target, record, fenced, writes, crashes>>

\* blob.rs ingest_blob phase 3: rename the staged replica into its shard ...
IngestRename ==
    /\ Writing /\ phase = "ingestRename"
    /\ blobLive' = [blobLive EXCEPT ![idx] = @ \cup {ver}]
    /\ phase' = "ingestSync"
    /\ UNCHANGED <<ptrLive, ptrDur, blobDur, mode, task, ver, idx, keep, journalled, txPrep,
                   target, record, fenced, writes, crashes>>

\* ... then fsync the shard, replica by replica.
IngestSync ==
    /\ Writing /\ phase = "ingestSync"
    /\ blobDur' = [blobDur EXCEPT ![idx] = blobLive[idx]]
    /\ IF idx < NBlob
       THEN idx' = idx + 1 /\ phase' = "ingestRename"
       ELSE idx' = 1 /\ phase' = IF task = "txwrite" THEN "txPrepare"
                                  ELSE IF Fix = "record" THEN "recWrite" ELSE "ptrRename"
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, mode, task, ver, keep, journalled, txPrep,
                   target, record, fenced, writes, crashes>>

\* "record": durably name the version about to be published (append + fsync). With
\* RecordDetail the append and the fsync are two steps, and a failed fsync ends the
\* write with the record in the file and not yet durable.
RecWrite ==
    /\ task = "write" /\ phase = "recWrite"
    /\ IF RecordDetail
          THEN /\ record' = [record EXCEPT !.live = ver]
               /\ phase' = IF Control = "record-not-synced" THEN AfterRecord ELSE "recSync"
          ELSE /\ record' = [live |-> ver, dur |-> ver]
               /\ phase' = AfterRecord
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, mode, task, ver, idx, keep,
                   journalled, txPrep, target, fenced, writes, crashes>>

RecSync ==
    /\ task = "write" /\ phase = "recSync"
    /\ record' = [record EXCEPT !.dur = record.live]
    /\ phase' = AfterRecord
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, mode, task, ver, idx, keep,
                   journalled, txPrep, target, fenced, writes, crashes>>

\* Only under Control = "record-cleared-early": the record goes before the pointers move.
RecClearEarly ==
    /\ task = "write" /\ phase = "recClearEarly"
    /\ record' = NoRecord
    /\ phase' = "ptrRename"
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, mode, task, ver, idx, keep,
                   journalled, txPrep, target, fenced, writes, crashes>>

\* namespace.rs write_pointer: every replica staged first (a failure there is
\* Fail at the first rename), then per replica rename into place ...
PtrRename ==
    /\ Writing /\ phase = "ptrRename"
    /\ ptrLive' = [ptrLive EXCEPT ![idx] = ver]
    /\ phase' = "ptrSync"
    /\ UNCHANGED <<ptrDur, blobLive, blobDur, mode, task, ver, idx, keep, journalled, txPrep,
                   target, record, fenced, writes, crashes>>

\* ... and fsync the entry's parent.
PtrSync ==
    /\ Writing /\ phase = "ptrSync"
    /\ ptrDur' = [ptrDur EXCEPT ![idx] = ptrLive[idx]]
    /\ IF idx < NPtr
       THEN idx' = idx + 1 /\ phase' = "ptrRename"
       ELSE idx' = 1 /\ phase' = IF task = "txwrite" THEN "txEnd"
                                  ELSE IF Fix = "record" /\ Control # "record-cleared-early"
                                          THEN "recClear" ELSE "collRead"
    /\ UNCHANGED <<ptrLive, blobLive, blobDur, mode, task, ver, keep, journalled, txPrep,
                   target, record, fenced, writes, crashes>>

\* "record": the publication is complete and durable, so the record goes
\* (truncate + fsync).
RecClear ==
    /\ task = "write" /\ phase = "recClear"
    /\ IF RecordDetail
          THEN /\ record' = [record EXCEPT !.live = None]
               /\ phase' = "recClearSync"
          ELSE /\ record' = NoRecord
               /\ phase' = "collRead"
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, mode, task, ver, idx, keep,
                   journalled, txPrep, target, fenced, writes, crashes>>

RecClearSync ==
    /\ task = "write" /\ phase = "recClearSync"
    /\ record' = [record EXCEPT !.dur = None]
    /\ phase' = "collRead"
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, mode, task, ver, idx, keep,
                   journalled, txPrep, target, fenced, writes, crashes>>

\* What follows a settled record: a write goes on to ingest, a recovery rolls a prepared
\* transaction forward, then goes to collection when a transaction ended, and otherwise it
\* is over.
AfterSettle ==
    CASE Writing        -> phase' = "ingestRename" /\ UNCHANGED <<task, mode>>
      [] task = "check"   -> phase' = "chkRead" /\ UNCHANGED <<task, mode>>
      [] task = "collect" -> phase' = "collRead" /\ UNCHANGED <<task, mode>>
      [] Maintaining    -> phase' = "repRead" /\ UNCHANGED <<task, mode>>
      [] txPrep # None  -> phase' = "txRfRename" /\ UNCHANGED <<task, mode>>
      [] journalled     -> phase' = "collRead" /\ UNCHANGED <<task, mode>>
      [] OTHER          -> Finish

-----------------------------------------------------------------------------
(* "record": roll every replica forward to the recorded version, then      *)
(* clear the record. Run by a write before its own work and by a reopen    *)
(* before recovery.                                                        *)
RfRecSync ==
    /\ task # "none" /\ phase = "rfRecSync"
    /\ record' = [record EXCEPT !.dur = record.live]
    /\ IF record.live # None
          THEN phase' = "rfRename" /\ UNCHANGED <<task, mode>>
          ELSE AfterSettle
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, ver, idx, keep, journalled, txPrep,
                   target, fenced, writes, crashes>>

RfRename ==
    /\ task # "none" /\ phase = "rfRename"
    /\ ptrLive' = [ptrLive EXCEPT ![idx] = record.live]
    /\ phase' = "rfSync"
    /\ UNCHANGED <<ptrDur, blobLive, blobDur, mode, task, ver, idx, keep, journalled, txPrep,
                   target, record, fenced, writes, crashes>>

RfSync ==
    /\ task # "none" /\ phase = "rfSync"
    /\ ptrDur' = [ptrDur EXCEPT ![idx] = ptrLive[idx]]
    /\ IF idx < NPtr
       THEN idx' = idx + 1 /\ phase' = "rfRename"
       ELSE idx' = 1 /\ phase' = "rfClear"
    /\ UNCHANGED <<ptrLive, blobLive, blobDur, mode, task, ver, keep, journalled, txPrep,
                   target, record, fenced, writes, crashes>>

RfClear ==
    /\ task # "none" /\ phase = "rfClear"
    /\ IF RecordDetail
          THEN /\ record' = [record EXCEPT !.live = None]
               /\ phase' = "rfClearSync"
               /\ UNCHANGED <<task, mode>>
          ELSE /\ record' = NoRecord
               /\ AfterSettle
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, ver, idx, keep, journalled, txPrep,
                   target, fenced, writes, crashes>>

RfClearSync ==
    /\ task # "none" /\ phase = "rfClearSync"
    /\ record' = [record EXCEPT !.dur = None]
    /\ AfterSettle
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, ver, idx, keep, journalled, txPrep,
                   target, fenced, writes, crashes>>

-----------------------------------------------------------------------------
(* A transaction's write of the document, and recovery of one.             *)

\* The prepare record is in the log, which is durable once appended.
TxPrepare ==
    /\ task = "txwrite" /\ phase = "txPrepare"
    /\ txPrep' = ver
    /\ phase' = "ptrRename"
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, mode, task, ver, idx, keep, journalled,
                   target, record, fenced, writes, crashes>>

\* The end record follows the last swap, and what the write superseded is then collected.
TxEnd ==
    /\ task = "txwrite" /\ phase = "txEnd"
    /\ txPrep' = None
    /\ journalled' = TRUE
    /\ phase' = "collRead"
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, mode, task, ver, idx, keep,
                   target, record, fenced, writes, crashes>>

\* Recovery sets every replica to the version the prepare record names, syncs each, and
\* then writes the end record.
TxRfRename ==
    /\ task = "recover" /\ phase = "txRfRename"
    /\ ptrLive' = [ptrLive EXCEPT ![idx] = txPrep]
    /\ phase' = "txRfSync"
    /\ UNCHANGED <<ptrDur, blobLive, blobDur, mode, task, ver, idx, keep, journalled, txPrep,
                   target, record, fenced, writes, crashes>>

TxRfSync ==
    /\ task = "recover" /\ phase = "txRfSync"
    /\ ptrDur' = [ptrDur EXCEPT ![idx] = ptrLive[idx]]
    /\ IF idx < NPtr
       THEN idx' = idx + 1 /\ phase' = "txRfRename"
       ELSE idx' = 1 /\ phase' = "txRfEnd"
    /\ UNCHANGED <<ptrLive, blobLive, blobDur, mode, task, ver, keep, journalled, txPrep,
                   target, record, fenced, writes, crashes>>

TxRfEnd ==
    /\ task = "recover" /\ phase = "txRfEnd"
    /\ txPrep' = None
    /\ journalled' = TRUE
    /\ phase' = "collRead"
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, mode, task, ver, idx, keep,
                   target, record, fenced, writes, crashes>>

\* A holder that is told recovery is required runs it without reopening the store.
Recover ==
    /\ mode = "up" /\ task = "none" /\ txPrep # None
    /\ task' = "recover" /\ idx' = 1 /\ phase' = RecoverPhase
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, mode, ver, keep, journalled, txPrep,
                   target, record, fenced, writes, crashes>>

-----------------------------------------------------------------------------
(* A check, a repair and a scrub, and the loss of a pointer replica.       *)

\* A precondition check reads the document and ends.
ChkRead ==
    /\ task = "check" /\ phase = "chkRead"
    /\ Finish
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, ver, idx, keep, journalled, txPrep,
                   target, record, fenced, writes, crashes>>

\* What follows a replica a repair has looked at: the next one, or the end.
RepNext ==
    IF idx < NPtr
    THEN idx' = idx + 1 /\ phase' = "repRename" /\ UNCHANGED <<task, mode>>
    ELSE Finish /\ UNCHANGED idx

\* store/repair.rs repair_pointer: read the vote again, and write its version to every replica
\* that has none.
RepRead ==
    /\ Maintaining /\ phase = "repRead"
    /\ LET m == Majority(ptrLive)
       IN  IF m \in Versions /\ \E r \in Ptr : ptrLive[r] = None
           THEN keep' = {m} /\ idx' = 1 /\ phase' = "repRename" /\ UNCHANGED <<task, mode>>
           ELSE Finish /\ UNCHANGED <<keep, idx>>
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, ver, journalled, txPrep, target, record,
                   fenced, writes, crashes>>

RepRename ==
    /\ Maintaining /\ phase = "repRename"
    /\ IF ptrLive[idx] = None
       THEN /\ ptrLive' = [ptrLive EXCEPT ![idx] = CHOOSE v \in keep : TRUE]
            /\ phase' = "repSync"
            /\ UNCHANGED <<task, mode, idx>>
       ELSE ptrLive' = ptrLive /\ RepNext
    /\ UNCHANGED <<ptrDur, blobLive, blobDur, ver, keep, journalled, txPrep, target, record,
                   fenced, writes, crashes>>

RepSync ==
    /\ Maintaining /\ phase = "repSync"
    /\ ptrDur' = [ptrDur EXCEPT ![idx] = ptrLive[idx]]
    /\ RepNext
    /\ UNCHANGED <<ptrLive, blobLive, blobDur, ver, keep, journalled, txPrep, target, record,
                   fenced, writes, crashes>>

\* EXTERNAL: a pointer replica of a document that is whole and settled is lost, live and
\* durable. The loss is the first fault; at most one replica is missing at a time.
Damage(r) ==
    /\ Maintenance
    /\ mode = "up" /\ task = "none" /\ txPrep = None
    /\ record = NoRecord /\ ptrLive = ptrDur /\ Unanimous(ptrLive)
    /\ \A p \in Ptr : ptrLive[p] # None
    /\ ptrLive' = [ptrLive EXCEPT ![r] = None]
    /\ ptrDur' = [ptrDur EXCEPT ![r] = None]
    /\ UNCHANGED <<blobLive, blobDur, mode, task, phase, ver, idx, keep, journalled, txPrep,
                   target, record, fenced, writes, crashes>>

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
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, ver, journalled, txPrep, target, record,
                   fenced, writes, crashes>>

\* namespace.rs sync_pointer_entry: fsync each replica's entry parent.
CollSync ==
    /\ task # "none" /\ phase = "collSync"
    /\ ptrDur' = [ptrDur EXCEPT ![idx] = ptrLive[idx]]
    /\ IF idx < NPtr
       THEN idx' = idx + 1 /\ phase' = "collSync"
       ELSE idx' = 1 /\ phase' = "collRemove"
    /\ UNCHANGED <<ptrLive, blobLive, blobDur, mode, task, ver, keep, journalled, txPrep,
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
    /\ UNCHANGED <<ptrLive, ptrDur, blobDur, ver, keep, journalled, txPrep, target, record,
                   fenced, writes, crashes>>

CollFsync ==
    /\ task # "none" /\ phase = "collFsync"
    /\ blobDur' = [blobDur EXCEPT ![idx] = blobLive[idx]]
    /\ NextBlobOrFinish
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, ver, keep, journalled, txPrep, target, record,
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
    /\ record' = [live |-> record.dur, dur |-> record.dur]
    /\ crashes' = crashes + 1
    /\ UNCHANGED <<ptrDur, blobDur, ver, idx, keep, journalled, txPrep, target, writes>>

\* The last handle is dropped without a crash.
Close ==
    /\ mode = "up" /\ task = "none"
    /\ mode' = "down"
    /\ fenced' = FALSE
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, task, phase, ver, idx, keep,
                   journalled, txPrep, target, record, writes, crashes>>

\* store.rs Store::open -> recover: a "record" roll-forward first, then
\* collection is re-run for every ended transaction still in the log.
Open ==
    /\ mode = "down"
    /\ idx' = 1
    /\ IF Settles \/ txPrep # None \/ journalled
          THEN mode' = "recovering" /\ task' = "recover" /\ phase' = RecoverPhase
          ELSE mode' = "up" /\ UNCHANGED <<task, phase>>
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, ver, keep, journalled, txPrep, target,
                   record, fenced, writes, crashes>>

\* The quiescence reset empties the log.
Quiesce ==
    /\ mode = "up" /\ task = "none" /\ journalled /\ txPrep = None
    /\ journalled' = FALSE
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, mode, task, phase, ver, idx,
                   keep, txPrep, target, record, fenced, writes, crashes>>

Next ==
    \/ \E v \in Versions : StartWrite(v)
    \/ \E v \in Versions : StartTx(v)
    \/ ResStart \/ ResRename \/ ResSync
    \/ IngestRename \/ IngestSync \/ RecWrite \/ RecSync \/ RecClearEarly
    \/ PtrRename \/ PtrSync \/ RecClear \/ RecClearSync
    \/ RfRecSync \/ RfRename \/ RfSync \/ RfClear \/ RfClearSync
    \/ TxPrepare \/ TxEnd \/ TxRfRename \/ TxRfSync \/ TxRfEnd \/ Recover
    \/ \E k \in {"check", "collect", "repair", "scrub"} : StartOp(k)
    \/ ChkRead \/ RepRead \/ RepRename \/ RepSync \/ \E r \in Ptr : Damage(r)
    \/ CollRead \/ CollSync \/ CollRemove \/ CollFsync
    \/ Fail \/ Crash \/ Close \/ Open \/ Quiesce

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
(* Properties.                                                             *)

\* Audience: client. After any crash, the version a document is published at still has its
\* content. Shows through: a read of a document after a crash.
\* DANGLING-DURABLE: whatever a crash would leave, a pointer majority names a
\* blob that survives on some replica.
DurablePointerHasBlob ==
    Majority(ptrDur) \in Versions => \E b \in Blob : Majority(ptrDur) \in blobDur[b]

\* Audience: client. The version a document is published at always has its content for a reader.
\* Shows through: a read of a document.
\* The same for the live state a reader sees.
LivePointerHasBlob ==
    Majority(ptrLive) \in Versions => \E b \in Blob : Majority(ptrLive) \in blobLive[b]

\* Audience: client. A read is never told to wait for a writer that does not exist. Shows through:
\* the outcome of a read.
\* STRANDED-VOTE: with no operation in progress, the namespace has a majority --
\* a version or absence -- so a read is never told to wait for a writer that
\* does not exist.
IdleImpliesSettled ==
    (mode = "up" /\ task = "none" /\ txPrep = None) => Majority(ptrLive) # NoMajority

\* Audience: client. As IdleImpliesSettled, except while a replica is lost: a publication that fails
\* part-way in a document that has lost a replica leaves three values, two faults that the
\* guards are not asked to absorb. Shows through: the outcome of a read.
IdleImpliesSettledUnlessAReplicaIsLost ==
    (mode = "up" /\ task = "none" /\ txPrep = None /\ \A r \in Ptr : ptrLive[r] # None)
        => Majority(ptrLive) # NoMajority

\* Audience: internal.
\* Under the record guard a pointer replica is renamed, by a write or by a settle, only
\* once the record naming the version it is set to is durable, so a crash cannot leave
\* renamed replicas with no record to finish them.
PointersMoveOnlyUnderADurableRecord ==
    [][(Fix = "record" /\ crashes' = crashes /\ task # "txwrite" /\ phase # "txRfRename"
        /\ phase # "repRename")
          => \A r \in Ptr : ptrLive'[r] # ptrLive[r] /\ ptrLive'[r] # None
                              => record.dur = ptrLive'[r]]_vars

\* Audience: internal.
\* The record is cleared only once the version it names is durable on every
\* replica. A crash loses what was not synced and is not a clear.
TheRecordGoesOnlyOnceItsVersionIsDurable ==
    [][(record.live # None /\ record'.live = None /\ crashes' = crashes)
          => \A r \in Ptr : ptrDur'[r] = record.live]_vars

\* Audience: client. A precondition check never reads a document with a record pending that
\* will replace the value it reads. Shows through: the outcome of a compare-and-set.
ACheckReadsASettledRecord ==
    (Fix = "record" /\ task = "check" /\ phase = "chkRead") => record = NoRecord

\* Audience: internal.
\* A repair or a scrub writes only the vote's version, and only to a replica that has none.
RepairOnlyFillsAMissingReplicaWithTheVote ==
    [][(phase = "repRename" /\ crashes' = crashes)
          => \A r \in Ptr : ptrLive'[r] # ptrLive[r]
              => ptrLive[r] = None /\ ptrLive'[r] = Majority(ptrLive)]_vars

-----------------------------------------------------------------------------
(* Witnesses: each is violated by a reachable state it names.              *)

Witness_AFailedRecordSyncLeavesTheRecordUnsynced ==
    ~(task = "none" /\ record.live # None /\ record.live # record.dur)

Witness_AStaleRecordIsLeftByAFailedClear ==
    ~(task = "none" /\ record.live = None /\ record.dur # None)

Witness_AWriteFollowsAFailedClear ==
    ~(task = "write" /\ phase = "recWrite" /\ record.live = None /\ record.dur # None)

Witness_ALoggedWriteFollowsAFailedClear ==
    ~(task = "txwrite" /\ phase = "txPrepare" /\ record.live = None /\ record.dur # None)

Witness_APreparedTransactionIsRolledForward ==
    ~(task = "recover" /\ phase = "txRfEnd")

Witness_AnOperationSettlesAPendingRecord ==
    ~(task \in {"check", "collect", "repair", "scrub"} /\ phase = "rfRename")

Witness_APointerReplicaIsLost ==
    ~(\E r \in Ptr : ptrLive[r] = None)

Witness_ARepairFillsAMissingReplica ==
    ~(Maintaining /\ phase = "repSync")

=============================================================================
