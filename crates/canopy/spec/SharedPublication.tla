\* Copyright (c) Michael Grier.
------------------------- MODULE SharedPublication -------------------------
(***************************************************************************)
(* The publication of several documents by processes that share one store *)
(* under the coordination contract (crates/canopy/DESIGN-NOTES.md,         *)
(* "Composing the publication model with the coordination contract").      *)
(*                                                                         *)
(* It extends Coordination.tla and adds the documents. Every step is one   *)
(* of the contract's actions conjoined with what it does to the documents, *)
(* or a step on the documents alone that leaves every variable of the      *)
(* contract as it was, so every behaviour here is a behaviour of the       *)
(* contract with documents added. The configurations name a scenario of    *)
(* writers with no registry, no upgrade, no clones, no reads and no        *)
(* discovery, and the contract's actions for those are left out.           *)
(*                                                                         *)
(* A document is PointerPublication.tla's: pointer replicas, each with a   *)
(* live and a durable value, blob replicas holding sets of versions, and a *)
(* publication record with a live and a durable value. A write publishes   *)
(* the version its holder's generation names. Only the holder of writer   *)
(* exclusion works on a document, so the work in progress is one set of   *)
(* variables, which a death of the holder or a crash discards.             *)
(*                                                                         *)
(* An unlogged write (Txn false) picks a document, ingests the blob, then  *)
(* under the "record" guard appends and syncs the record, renames and      *)
(* syncs each pointer replica, and clears and syncs the record. The first  *)
(* step that decides the write's outcome -- the record's append under the  *)
(* "record" guard, otherwise the first pointer rename -- is the contract's *)
(* first effect, and the step that completes the durable work is its last. *)
(* A logged write (Txn) ingests while the transaction is begun, writes the *)
(* prepare record naming the document and version, swaps the replicas and *)
(* writes no publication record. Either then collects what it superseded. *)
(* With MixedWrites each write chooses its form when it begins.            *)
(* A failure is the contract's: before the first effect the operation ends *)
(* without one, and after it the operation fails unresolved.               *)
(*                                                                         *)
(* The guards' own work at the start of a write -- settling a pending      *)
(* record, or resolving the replicas -- is not modelled. The contract's    *)
(* fence makes every acquirer after an unresolved failure, a death or a    *)
(* restart run recovery first, and AHealthyHolderFindsEveryDocumentSettled *)
(* checks that a holder who is not told recovery is required finds nothing *)
(* for that work to do.                                                    *)
(*                                                                         *)
(* Recovery does the guard's work for every document, since no durable     *)
(* state names the one an unlogged write was publishing: under "record" it *)
(* syncs each record file, rolls forward what it finds and clears it;      *)
(* under "resolve" it sets every replica of every document to its majority *)
(* and syncs it; under "none" it does nothing. A prepared transaction is   *)
(* rolled forward from its prepare record while the contract makes its    *)
(* swaps.                                                                  *)
(*                                                                         *)
(* PubControl selects a fault to confirm the properties detect it:         *)
(*   "none"                       the composition.                         *)
(*   "open-settles"               a handle settles every record when it    *)
(*                                attaches.                                *)
(*   "recovery-skips-a-document"  recovery does its work on the first      *)
(*                                document only.                           *)
(*   "recovery-leaves-the-documents"                                       *)
(*                                recovery does no work on any document.   *)
(***************************************************************************)
EXTENDS Coordination

CONSTANTS
    Docs,        \* the documents, 1..n
    NPtr,        \* pointer replicas of each document
    NBlob,       \* blob replicas of each document
    Fix,         \* the STRANDED-VOTE guard: "record", "resolve" or "none"
    PubControl   \* a fault in the publication or its recovery

ASSUME Docs = 1..Cardinality(Docs) /\ Docs # {}
ASSUME NPtr \in Nat /\ NPtr % 2 = 1 /\ NBlob \in Nat \ {0}
ASSUME Fix \in {"record", "resolve", "none"}
ASSUME PubControl \in {"none", "open-settles", "recovery-skips-a-document",
                       "recovery-leaves-the-documents"}
ASSUME UnloggedRecovery = ~Txn

Ptr == 1..NPtr
Blob == 1..NBlob
Vers == 0..Acquisitions
LastDoc == Cardinality(Docs)
NoRecord == [live |-> NoVal, dur |-> NoVal]
NoPrep == [doc |-> 0, ver |-> NoVal]
NoMajority == -2

VARIABLES
    ptrLive,     \* [Docs -> [Ptr -> Vers]]: each pointer replica's live value
    ptrDur,      \* [Docs -> [Ptr -> Vers]]: what a crash would leave
    blobLive,    \* [Docs -> [Blob -> SUBSET Vers]]: versions on each blob replica
    blobDur,     \* [Docs -> [Blob -> SUBSET Vers]]
    record,      \* [Docs -> [live, dur]]: each document's publication record
    prep,        \* the document and version the prepare record names, or NoPrep
    wph,         \* the next step of the work in progress
    wdoc,        \* the document it is on, or 0
    wix,         \* the replica it is at
    wkeep,       \* the versions a collection keeps
    rkinds       \* ghost, MixedWrites: which kinds of write recovery has rolled forward

docs == <<ptrLive, ptrDur, blobLive, blobDur, record, prep>>
work == <<wph, wdoc, wix, wkeep>>
spvars == <<vars, docs, work, rkinds>>

Holding(f, v) == Cardinality({r \in Ptr : f[r] = v})
Majority(f) ==
    IF \E v \in Vers : 2 * Holding(f, v) > NPtr
    THEN CHOOSE v \in Vers : 2 * Holding(f, v) > NPtr
    ELSE NoMajority
Unanimous(f) == \A r \in Ptr : f[r] = f[1]

\* The version the holder p publishes.
V(p) == mygen[p]

\* p holds writer exclusion and so is the only one working on a document.
W(p) == lockHolder = p

Phases == {"none", "ingRen", "ingSync", "staged", "recWrite", "recSync", "ptrRen",
           "ptrSync", "recClr", "recClrSync", "collRead", "collSync", "collRem",
           "collFsync", "done", "rvRecSync", "rvRen", "rvSync", "rvClr",
           "rvClrSync", "rsRen", "rsSync", "rvDone", "txRen", "txSync", "txDone"}

NoWork == wph' = "none" /\ wdoc' = 0 /\ wix' = 1 /\ wkeep' = {}

\* Audience: internal.
SpTypeOK ==
    /\ TypeOK
    /\ ptrLive \in [Docs -> [Ptr -> Vers]]
    /\ ptrDur \in [Docs -> [Ptr -> Vers]]
    /\ blobLive \in [Docs -> [Blob -> SUBSET Vers]]
    /\ blobDur \in [Docs -> [Blob -> SUBSET Vers]]
    /\ record \in [Docs -> [live : Vers \cup {NoVal}, dur : Vers \cup {NoVal}]]
    /\ prep \in {NoPrep} \cup [doc : Docs, ver : Vers]
    /\ wph \in Phases
    /\ wdoc \in Docs \cup {0}
    /\ wix \in Ptr \cup Blob
    /\ wkeep \in SUBSET Vers
    /\ rkinds \in SUBSET {"unlogged", "logged"}

SpInit ==
    /\ Init
    /\ ptrLive = [d \in Docs |-> [r \in Ptr |-> 0]]
    /\ ptrDur = ptrLive
    /\ blobLive = [d \in Docs |-> [k \in Blob |-> {0}]]
    /\ blobDur = blobLive
    /\ record = [d \in Docs |-> NoRecord]
    /\ prep = NoPrep
    /\ wph = "none" /\ wdoc = 0 /\ wix = 1 /\ wkeep = {}
    /\ rkinds = {}

-----------------------------------------------------------------------------
(* The contract's steps that touch no document.                            *)

Quiet(A) == A /\ UNCHANGED <<docs, work>>

\* Every record rolled forward and cleared at once, which only the control does.
SettleAll ==
    /\ ptrLive' = [d \in Docs |-> IF record[d].live # NoVal
                                     THEN [r \in Ptr |-> record[d].live]
                                     ELSE ptrLive[d]]
    /\ ptrDur' = [d \in Docs |-> IF record[d].live # NoVal
                                    THEN [r \in Ptr |-> record[d].live]
                                    ELSE ptrDur[d]]
    /\ record' = [d \in Docs |-> NoRecord]
    /\ UNCHANGED <<blobLive, blobDur, prep>>

SAdmit(p) ==
    /\ Admit(p)
    /\ IF PubControl = "open-settles" THEN SettleAll ELSE UNCHANGED docs
    /\ UNCHANGED work

-----------------------------------------------------------------------------
(* The contract's steps that end or fail the work.                         *)

\* An operation that ends before its first effect.
SEndEmpty(p) ==
    /\ EndEmpty(p)
    /\ wph' = "done" /\ wdoc' = 0 /\ wix' = 1 /\ wkeep' = {}
    /\ UNCHANGED docs

SAbort(p) ==
    /\ Abort(p)
    /\ wph' = "done" /\ wdoc' = 0 /\ wix' = 1 /\ wkeep' = {}
    /\ UNCHANGED docs

SFailUnresolved(p) == FailUnresolved(p) /\ NoWork /\ UNCHANGED docs

\* The closing raise begins once the work, collection included, is over.
SBeginClose(p) == BeginClose(p) /\ wph = "done" /\ NoWork /\ UNCHANGED docs

\* The prepare record names the document and version; one whose append failed
\* may or may not be in the log, and the record says the same as the log.
SPrepare(p) ==
    /\ Prepare(p)
    /\ wph = "staged"
    /\ prep' = [doc |-> wdoc, ver |-> V(p)]
    /\ wph' = "ptrRen" /\ wix' = 1
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, record, wdoc, wkeep>>

SPrepareFails(p) ==
    /\ PrepareFails(p)
    /\ wph = "staged"
    /\ prep' = IF log' = "prepared" THEN [doc |-> wdoc, ver |-> V(p)] ELSE prep
    /\ NoWork
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, record>>

\* A transaction begins before its write picks a document.
SBegin(p) == Begin(p) /\ wph = "none" /\ UNCHANGED <<docs, work>>

SIngest(p) == Ingest(p) /\ wph = "staged" /\ UNCHANGED <<docs, work>>

SEnd(p) ==
    /\ End(p)
    /\ prep' = NoPrep
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, record, work>>

SRecSwap1(p) == RecSwap1(p) /\ wph = "txDone" /\ NoWork /\ UNCHANGED docs

SRecEnd(p) ==
    /\ RecEnd(p)
    /\ wph = "rvDone"
    /\ prep' = NoPrep
    /\ NoWork
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, record>>

SRecStop(p) == RecStop(p) /\ NoWork /\ UNCHANGED docs

\* EXTERNAL: a process dies. The documents stay as the live filesystem has them,
\* and its work in progress is gone with it.
SDie(p) ==
    /\ Die(p)
    /\ IF lockHolder = p THEN NoWork ELSE UNCHANGED work
    /\ UNCHANGED docs

\* EXTERNAL: power loss. Every document reverts to what was durable.
SCrash ==
    /\ Crash
    /\ ptrLive' = ptrDur
    /\ blobLive' = blobDur
    /\ record' = [d \in Docs |-> [live |-> record[d].dur, dur |-> record[d].dur]]
    /\ NoWork
    /\ UNCHANGED <<ptrDur, blobDur, prep>>

-----------------------------------------------------------------------------
(* A write's work on its document.                                         *)

Pick(p, d) ==
    /\ W(p) /\ wph = "none"
    /\ pc[p] \in (IF Txn THEN {"begun"}
                  ELSE IF MixedWrites THEN {"begun", "open"} ELSE {"open"})
    /\ wdoc' = d /\ wph' = "ingRen" /\ wix' = 1
    /\ UNCHANGED <<vars, docs, wkeep>>

IngRen(p) ==
    /\ W(p) /\ wph = "ingRen"
    /\ blobLive' = [blobLive EXCEPT ![wdoc][wix] = @ \cup {V(p)}]
    /\ wph' = "ingSync"
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobDur, record, prep, wdoc, wix, wkeep>>

\* A transaction's write ingests while it is begun.
AfterIngest(p) ==
    IF pc[p] = "begun" THEN "staged" ELSE IF Fix = "record" THEN "recWrite" ELSE "ptrRen"

IngSync(p) ==
    /\ W(p) /\ wph = "ingSync"
    /\ blobDur' = [blobDur EXCEPT ![wdoc][wix] = blobLive[wdoc][wix]]
    /\ IF wix < NBlob
          THEN wix' = wix + 1 /\ wph' = "ingRen"
          ELSE wix' = 1 /\ wph' = AfterIngest(p)
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, record, prep, wdoc, wkeep>>

\* "record": the append decides the outcome, so it is the first effect.
RecWrite(p) ==
    /\ W(p) /\ wph = "recWrite"
    /\ FirstEffect(p)
    /\ record' = [record EXCEPT ![wdoc].live = V(p)]
    /\ wph' = "recSync"
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, prep, wdoc, wix, wkeep>>

RecSync(p) ==
    /\ W(p) /\ wph = "recSync"
    /\ record' = [record EXCEPT ![wdoc].dur = record[wdoc].live]
    /\ wph' = "ptrRen"
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, blobDur, prep, wdoc, wix, wkeep>>

\* The first rename is the first effect when nothing before it was.
PtrRen(p) ==
    /\ W(p) /\ wph = "ptrRen"
    /\ IF pc[p] \in {"open", "prepared"} THEN FirstEffect(p) ELSE UNCHANGED vars
    /\ ptrLive' = [ptrLive EXCEPT ![wdoc][wix] = V(p)]
    /\ wph' = "ptrSync"
    /\ UNCHANGED <<ptrDur, blobLive, blobDur, record, prep, wdoc, wix, wkeep>>

PtrSync(p) ==
    /\ W(p) /\ wph = "ptrSync"
    /\ ptrDur' = [ptrDur EXCEPT ![wdoc][wix] = ptrLive[wdoc][wix]]
    /\ IF wix < NPtr
          THEN wix' = wix + 1 /\ wph' = "ptrRen" /\ UNCHANGED vars
       ELSE IF Fix = "record" /\ ~TxnOp
          THEN wix' = 1 /\ wph' = "recClr" /\ UNCHANGED vars
          ELSE wix' = 1 /\ wph' = "collRead" /\ LastEffect(p)
    /\ UNCHANGED <<ptrLive, blobLive, blobDur, record, prep, wdoc, wkeep>>

RecClr(p) ==
    /\ W(p) /\ wph = "recClr"
    /\ record' = [record EXCEPT ![wdoc].live = NoVal]
    /\ wph' = "recClrSync"
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, blobDur, prep, wdoc, wix, wkeep>>

\* The clear is durable: the write's work is complete.
RecClrSync(p) ==
    /\ W(p) /\ wph = "recClrSync"
    /\ LastEffect(p)
    /\ record' = [record EXCEPT ![wdoc].dur = NoVal]
    /\ wph' = "collRead"
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, prep, wdoc, wix, wkeep>>

-----------------------------------------------------------------------------
(* Collection of what the write superseded, once its operation is done.    *)

CollRead(p) ==
    /\ W(p) /\ pc[p] = "wrote" /\ wph = "collRead"
    /\ LET m == Majority(ptrLive[wdoc])
       IN  IF m \in Vers
           THEN wkeep' = {m} /\ wph' = "collSync" /\ wix' = 1
           ELSE wph' = "done" /\ UNCHANGED <<wkeep, wix>>
    /\ UNCHANGED <<vars, docs, wdoc>>

\* The entry is synced on every replica before anything is removed.
CollSync(p) ==
    /\ W(p) /\ wph = "collSync"
    /\ ptrDur' = [ptrDur EXCEPT ![wdoc][wix] = ptrLive[wdoc][wix]]
    /\ IF wix < NPtr
          THEN wix' = wix + 1 /\ wph' = "collSync"
          ELSE wix' = 1 /\ wph' = "collRem"
    /\ UNCHANGED <<vars, ptrLive, blobLive, blobDur, record, prep, wdoc, wkeep>>

CollNext ==
    IF wix < NBlob
    THEN wix' = wix + 1 /\ wph' = "collRem"
    ELSE wix' = 1 /\ wph' = "done"

CollRem(p) ==
    /\ W(p) /\ wph = "collRem"
    /\ IF blobLive[wdoc][wix] \subseteq wkeep
          THEN CollNext /\ UNCHANGED blobLive
          ELSE /\ blobLive' = [blobLive EXCEPT ![wdoc][wix] = @ \cap wkeep]
               /\ wph' = "collFsync"
               /\ UNCHANGED wix
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobDur, record, prep, wdoc, wkeep>>

CollFsync(p) ==
    /\ W(p) /\ wph = "collFsync"
    /\ blobDur' = [blobDur EXCEPT ![wdoc][wix] = blobLive[wdoc][wix]]
    /\ CollNext
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, record, prep, wdoc, wkeep>>

\* A collection that fails leaves the rest of the garbage behind.
CollFails(p) ==
    /\ W(p) /\ wph \in {"collRead", "collSync", "collRem", "collFsync"}
    /\ pc[p] = "wrote"
    /\ wph' = "done"
    /\ UNCHANGED <<vars, docs, wdoc, wix, wkeep>>

-----------------------------------------------------------------------------
(* Recovery's work on the documents, before it ends the log.               *)

FirstRecoveryPhase ==
    IF PubControl = "recovery-leaves-the-documents" THEN "rvDone"
    ELSE CASE Fix = "record"  -> "rvRecSync"
           [] Fix = "resolve" -> "rsRen"
           [] OTHER           -> "rvDone"

RvBegin(p) ==
    /\ W(p) /\ pc[p] = "recEnd" /\ wph = "none"
    /\ wph' = FirstRecoveryPhase /\ wdoc' = 1 /\ wix' = 1
    /\ UNCHANGED <<vars, docs, wkeep>>

\* The document after this one, or the end of recovery's work on them.
NextDoc ==
    IF wdoc < (IF PubControl = "recovery-skips-a-document" THEN 1 ELSE LastDoc)
    THEN wdoc' = wdoc + 1 /\ wix' = 1 /\ wph' = FirstRecoveryPhase
    ELSE wdoc' = 0 /\ wix' = 1 /\ wph' = "rvDone"

\* "record": the record file is synced first, so a failed clear is seen, then
\* what it names is rolled forward and the record cleared.
RvRecSync(p) ==
    /\ W(p) /\ wph = "rvRecSync"
    /\ record' = [record EXCEPT ![wdoc].dur = record[wdoc].live]
    /\ IF record[wdoc].live # NoVal
          THEN wph' = "rvRen" /\ wix' = 1 /\ UNCHANGED wdoc
          ELSE NextDoc
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, blobDur, prep, wkeep>>

RvRen(p) ==
    /\ W(p) /\ wph = "rvRen"
    /\ ptrLive' = [ptrLive EXCEPT ![wdoc][wix] = record[wdoc].live]
    /\ wph' = "rvSync"
    /\ UNCHANGED <<vars, ptrDur, blobLive, blobDur, record, prep, wdoc, wix, wkeep>>

RvSync(p) ==
    /\ W(p) /\ wph = "rvSync"
    /\ ptrDur' = [ptrDur EXCEPT ![wdoc][wix] = ptrLive[wdoc][wix]]
    /\ IF wix < NPtr
          THEN wix' = wix + 1 /\ wph' = "rvRen"
          ELSE wix' = 1 /\ wph' = "rvClr"
    /\ UNCHANGED <<vars, ptrLive, blobLive, blobDur, record, prep, wdoc, wkeep>>

RvClr(p) ==
    /\ W(p) /\ wph = "rvClr"
    /\ record' = [record EXCEPT ![wdoc].live = NoVal]
    /\ wph' = "rvClrSync"
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, blobDur, prep, wdoc, wix, wkeep>>

RvClrSync(p) ==
    /\ W(p) /\ wph = "rvClrSync"
    /\ record' = [record EXCEPT ![wdoc].dur = NoVal]
    /\ NextDoc
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, blobDur, prep, wkeep>>

\* "resolve": every replica is set to the document's majority and synced.
RsRen(p) ==
    /\ W(p) /\ wph = "rsRen"
    /\ LET m == Majority(ptrLive[wdoc])
       IN  IF m \in Vers
           THEN ptrLive' = [ptrLive EXCEPT ![wdoc][wix] = m] /\ wph' = "rsSync"
                /\ UNCHANGED <<wdoc, wix>>
           ELSE UNCHANGED ptrLive /\ NextDoc
    /\ UNCHANGED <<vars, ptrDur, blobLive, blobDur, record, prep, wkeep>>

RsSync(p) ==
    /\ W(p) /\ wph = "rsSync"
    /\ ptrDur' = [ptrDur EXCEPT ![wdoc][wix] = ptrLive[wdoc][wix]]
    /\ IF wix < NPtr
          THEN wix' = wix + 1 /\ wph' = "rsRen" /\ UNCHANGED wdoc
          ELSE NextDoc
    /\ UNCHANGED <<vars, ptrLive, blobLive, blobDur, record, prep, wkeep>>

\* A prepared transaction is rolled forward from its prepare record while the
\* contract makes its swaps.
TxBegin(p) ==
    /\ W(p) /\ pc[p] = "recSwap1" /\ wph = "none"
    /\ wph' = "txRen" /\ wdoc' = prep.doc /\ wix' = 1
    /\ UNCHANGED <<vars, docs, wkeep>>

TxRen(p) ==
    /\ W(p) /\ wph = "txRen"
    /\ ptrLive' = [ptrLive EXCEPT ![wdoc][wix] = prep.ver]
    /\ wph' = "txSync"
    /\ UNCHANGED <<vars, ptrDur, blobLive, blobDur, record, prep, wdoc, wix, wkeep>>

TxSync(p) ==
    /\ W(p) /\ wph = "txSync"
    /\ ptrDur' = [ptrDur EXCEPT ![wdoc][wix] = ptrLive[wdoc][wix]]
    /\ IF wix < NPtr
          THEN wix' = wix + 1 /\ wph' = "txRen"
          ELSE wix' = 1 /\ wph' = "txDone"
    /\ UNCHANGED <<vars, ptrLive, blobLive, blobDur, record, prep, wdoc, wkeep>>

-----------------------------------------------------------------------------

\* Records, under MixedWrites, each kind of write whose residue recovery has rolled forward.
Kinds ==
    rkinds' = IF ~MixedWrites THEN rkinds
              ELSE rkinds \cup (IF wph' = "rvRen" THEN {"unlogged"} ELSE {})
                          \cup (IF wph' = "txRen" THEN {"logged"} ELSE {})

SpNextCore ==
    \/ \E p \in Procs :
          \/ Quiet(Exit(p)) \/ Quiet(BeginAttach(p)) \/ Quiet(TakeGateS(p))
          \/ Quiet(TakeReaderS(p)) \/ Quiet(DropGate(p)) \/ Quiet(TakeWriterS(p))
          \/ SAdmit(p)
          \/ Quiet(Request(p)) \/ Quiet(Lock(p)) \/ Quiet(SetFence(p))
          \/ Quiet(Record(p))
          \/ Quiet(BeginOpen(p)) \/ Quiet(EndOpen(p)) \/ Quiet(OpenFails(p))
          \/ SEndEmpty(p) \/ SFailUnresolved(p) \/ SBeginClose(p)
          \/ Quiet(EndClose(p)) \/ Quiet(CloseFails(p))
          \/ Quiet(Release(p)) \/ Quiet(Unlock(p))
          \/ SBegin(p) \/ SIngest(p) \/ SPrepare(p) \/ SPrepareFails(p)
          \/ SAbort(p) \/ SEnd(p) \/ Quiet(Sweep(p)) \/ Quiet(Quiesce(p))
          \/ Quiet(RecStart(p)) \/ Quiet(RecWhole(p)) \/ SRecSwap1(p)
          \/ Quiet(RecSwap2(p)) \/ SRecEnd(p) \/ Quiet(RecReset(p))
          \/ Quiet(RecSweep(p)) \/ Quiet(RecCount(p)) \/ Quiet(RecClear(p))
          \/ SRecStop(p)
          \/ SDie(p)
          \/ \E d \in Docs : Pick(p, d)
          \/ IngRen(p) \/ IngSync(p) \/ RecWrite(p) \/ RecSync(p)
          \/ PtrRen(p) \/ PtrSync(p) \/ RecClr(p) \/ RecClrSync(p)
          \/ CollRead(p) \/ CollSync(p) \/ CollRem(p) \/ CollFsync(p) \/ CollFails(p)
          \/ RvBegin(p) \/ RvRecSync(p) \/ RvRen(p) \/ RvSync(p) \/ RvClr(p)
          \/ RvClrSync(p) \/ RsRen(p) \/ RsSync(p)
          \/ TxBegin(p) \/ TxRen(p) \/ TxSync(p)
    \/ SCrash
    \/ Quiet(Terminal)

SpNext == SpNextCore /\ Kinds

SpSpec == SpInit /\ [][SpNext]_spvars

-----------------------------------------------------------------------------
(* Properties.                                                             *)

\* Audience: client. After any crash, the version a document is published at still has its
\* content. Shows through: a read of a document after a crash.
DurablePointerHasBlob ==
    \A d \in Docs :
        Majority(ptrDur[d]) \in Vers => \E k \in Blob : Majority(ptrDur[d]) \in blobDur[d][k]

\* Audience: client. The version a document is published at always has its content for a reader.
\* Shows through: a read of a document.
LivePointerHasBlob ==
    \A d \in Docs :
        Majority(ptrLive[d]) \in Vers => \E k \in Blob : Majority(ptrLive[d]) \in blobLive[d][k]

\* Audience: client. A read is never told to wait for a writer that does not exist. Shows through:
\* the outcome of a read.
\* STRANDED-VOTE: with nobody holding writer exclusion, every document has a majority.
EveryDocumentHasAMajority ==
    lockHolder = None => \A d \in Docs : Majority(ptrLive[d]) # NoMajority

\* Audience: client. Once recovery has run, the version a document reads as is the one a crash
\* would leave, and it changes only by a write. Shows through: a read after a write whose holder
\* was told recovery is required, once recovery has run.
OutcomeIsFixedOnceRecovered ==
    (lockHolder = None /\ owner = None /\ ~fence /\ trusted)
        => /\ prep = NoPrep
           /\ \A d \in Docs : /\ Majority(ptrLive[d]) = Majority(ptrDur[d])
                             /\ record[d] = NoRecord

\* Audience: internal.
\* A holder that is not told recovery is required finds every document settled, so the
\* guard's work at the start of a write -- a settle or a resolve -- has nothing to do.
AHealthyHolderFindsEveryDocumentSettled ==
    \A p \in Procs :
        (W(p) /\ pc[p] \in {"held", "open", "begun"} /\ wph = "none")
            => /\ prep = NoPrep
               /\ \A d \in Docs : /\ record[d] = NoRecord
                                 /\ ptrLive[d] = ptrDur[d]
                                 /\ Unanimous(ptrLive[d])

\* Audience: internal.
\* Only the holder of writer exclusion changes a document; a crash only loses what was not
\* durable. Attaching, the open of a handle, changes nothing.
OnlyAHolderChangesADocument ==
    [][(docs' # docs /\ crashes' = crashes) => \E p \in Procs : W(p)]_spvars

-----------------------------------------------------------------------------
(* Witnesses: each is violated by a reachable state or step it names.      *)

Witness_ARecoverySettlesARecordOfTheSecondDocument ==
    ~(wph = "rvRen" /\ wdoc = 2)

Witness_ARecoveryResolvesTheSecondDocument ==
    ~(wph = "rsSync" /\ wdoc = 2)

Witness_ARecoveryRollsATransactionForward ==
    ~(wph = "txRen")

Witness_AHolderDiesWithARecordPending ==
    ~(\E p \in Procs : pc[p] = "dead" /\ \E d \in Docs : record[d].dur # NoVal)

Witness_RecoveryRollsForwardBothKindsOfWrite ==
    ~(rkinds = {"unlogged", "logged"})

Witness_ARestartLosesTheFenceWithARecordPending ==
    ~(diedIn = "crash" /\ ~fence /\ \E d \in Docs : record[d].dur # NoVal)

=============================================================================
