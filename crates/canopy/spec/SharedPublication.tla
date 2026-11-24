\* Copyright (c) Michael Grier.
------------------------- MODULE SharedPublication -------------------------
(***************************************************************************)
(* The publication of several documents by processes that share one store *)
(* under the coordination contract (crates/canopy/DESIGN-NOTES.md,         *)
(* "Composing the publication model with the coordination contract").      *)
(*                                                                         *)
(* It extends Coordination.tla with documents and caller-driven recovery.  *)
(* Each recovery run remembers its phase and observed count. Each unit     *)
(* acquires, records and releases writer exclusion; another holder can     *)
(* intervene, and a changed count restarts the run's document work. The     *)
(* configurations name a scenario of                                      *)
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
(* syncs each record file, rolls forward what it finds, clears it and      *)
(* collects what the roll-forward superseded;                              *)
(* under "resolve" it sets every replica of every document to its majority *)
(* and syncs it; under "none" it does nothing. A prepared transaction is   *)
(* rolled forward from its prepare record while the contract makes its    *)
(* swaps. Each transaction's collection precedes completion's best-effort *)
(* log reset, count publication and fence clear. Empty transaction and     *)
(* collection phases take no unit.                                        *)
(*                                                                         *)
(* PubControl selects a fault to confirm the properties detect it:         *)
(*   "none"                       the composition.                         *)
(*   "open-settles"               a handle settles every record when it    *)
(*                                attaches.                                *)
(*   "recovery-skips-a-document"  recovery does its work on the first      *)
(*                                document only.                           *)
(*   "recovery-leaves-the-documents"                                       *)
(*                                recovery does no work on any document.   *)
(*   "resume-stale-recovery"       a run keeps progress after its saved    *)
(*                                count becomes stale.                    *)
(***************************************************************************)
EXTENDS Coordination

CONSTANTS
    Docs,        \* the documents, 1..n
    NPtr,        \* pointer replicas of each document
    NBlob,       \* blob replicas of each document
    MaxWrites,
    RecoverySchedule,
    InitialDirectoryDebt,
    InitialNodeRemoved,
    TransactionPreconditions,
    Fix,         \* the STRANDED-VOTE guard: "record", "resolve" or "none"
    PubControl   \* a fault in the publication or its recovery

ASSUME Docs = 1..Cardinality(Docs) /\ Docs # {}
ASSUME RecoverySchedule \in {"all", "stale-completion"}
ASSUME InitialDirectoryDebt \in Nat
ASSUME InitialNodeRemoved \in BOOLEAN
ASSUME TransactionPreconditions \in {"unconditional", "all"}
ASSUME RecoverySchedule = "stale-completion" => Procs = {1, 2}
ASSUME NPtr \in Nat /\ NPtr % 2 = 1 /\ NBlob \in Nat \ {0}
ASSUME Fix \in {"record", "resolve", "none"}
ASSUME PubControl \in {"none", "open-settles", "recovery-skips-a-document",
                       "recovery-leaves-the-documents", "resume-stale-recovery"}
ASSUME UnloggedRecovery = ~Txn

Ptr == 1..NPtr
Blob == 1..NBlob
Vers == Nat
LastDoc == Cardinality(Docs)
NoRecord == [live |-> NoVal, dur |-> NoVal]
NoPrep == <<>>
NoMajority == -2
Preconditions == {"unconditional", "digest", "absent"}
PreconditionChoices == IF TransactionPreconditions = "all"
                       THEN Preconditions ELSE {"unconditional"}
PreparedSwaps == [doc : Docs, ver : Vers, effect : {"write", "remove"},
                  precondition : Preconditions, expected : Vers \cup {NoVal}]
NamespaceRoot == <<1>>
DocumentEntry(d) == Append(NamespaceRoot, d)
DocumentBelow(d) == Append(DocumentEntry(d), 1)
NamespaceNodes == {NamespaceRoot} \cup {DocumentEntry(d) : d \in Docs}
                  \cup {DocumentBelow(d) : d \in Docs}
PathContains(parent, child) ==
    Len(parent) <= Len(child) /\ \A index \in 1..Len(parent) : parent[index] = child[index]

VARIABLES
    ptrLive,     \* [Docs -> [Ptr -> Vers]]: each pointer replica's live value
    ptrDur,      \* [Docs -> [Ptr -> Vers]]: what a crash would leave
    blobLive,    \* [Docs -> [Blob -> SUBSET Vers]]: versions on each blob replica
    blobDur,     \* [Docs -> [Blob -> SUBSET Vers]]
    record,      \* [Docs -> [live, dur]]: each document's publication record
    prep,        \* staged swaps, or the durable prepare record's swap sequence
    removeNodes, \* node paths applied after the prepare's document swaps
    nodeLive,    \* [Docs -> [Ptr -> BOOLEAN]]: each replica's live target node
    nodeDur,     \* what a crash would leave of the target nodes
    wph,         \* the next step of the work in progress
    wdoc,        \* the document it is on, or 0
    wix,         \* the replica it is at
    wkeep,       \* the versions a collection keeps
    txnDoc,
    runPhase,
    runCount,
    unitReady,
    requestRecovery,
    directoryLive,
    directoryDurable,
    directoryTorn,
    runDirectories,
    unitIntervened,
    writes,
    ordinaryRequests,
    rkinds       \* ghost, MixedWrites: which kinds of write recovery has rolled forward

docs == <<ptrLive, ptrDur, blobLive, blobDur, record, prep, removeNodes,
          nodeLive, nodeDur>>
work == <<wph, wdoc, wix, wkeep>>
spvars == <<vars, docs, work, txnDoc, runPhase, runCount, unitReady,
            requestRecovery, directoryLive, directoryDurable, directoryTorn,
            runDirectories, unitIntervened, writes, ordinaryRequests, rkinds>>

Holding(f, v) == Cardinality({r \in Ptr : f[r] = v})
Majority(f) ==
    IF \E v \in {f[replica] : replica \in Ptr} : 2 * Holding(f, v) > NPtr
    THEN CHOOSE v \in {f[replica] : replica \in Ptr} : 2 * Holding(f, v) > NPtr
    ELSE NoMajority
Unanimous(f) == \A r \in Ptr : f[r] = f[1]

\* The version the holder p publishes.
V(p) == mygen[p]

\* p holds writer exclusion and so is the only one working on a document.
W(p) == lockHolder = p

Phases == {"none", "planned", "planRemove", "planRemoveAtDocument", "planRemoveBelowDocument",
           "ingRen", "ingSync", "skipIng", "staged", "recWrite", "recSync", "ptrRen",
           "ptrSync", "recClr", "recClrSync", "collRead", "collSync", "collRem",
           "collFsync", "done", "rvRecSync", "rvRen", "rvSync", "rvClr",
           "rvClrSync", "rvCollRead", "rvCollSync", "rvCollRem", "rvCollFsync",
           "rsRen", "rsSync", "rvDone", "txRen", "txSync", "txDone",
           "rbCollRead", "rbCollSync", "rbCollRem", "rbCollFsync", "rbDone",
           "dirAppend", "dirAppendSync", "dirAppendRepair", "dirAppendTorn", "dirCreate",
           "dirRename", "nodeRen", "nodeSync", "nodeDone",
           "unitStart", "unitTxnDone", "claimsDone", "dirSync", "dirClear", "dirClearSync", "dirDone"}

RunPhases == {"none", "directories", "publication", "transaction", "collection", "claims", "completion"}

NoWork ==
    /\ wph' = "none" /\ wdoc' = 0 /\ wix' = 1 /\ wkeep' = {}
    /\ UNCHANGED <<prep, txnDoc>>

StagedDocuments == {prep[index].doc : index \in DOMAIN prep}
PlannedRemovalTargets == {removeNodes[index] : index \in DOMAIN removeNodes}
SwapSetHasUniqueTargets == Len(prep) = Cardinality(StagedDocuments)
DocumentAfterTransaction(document) ==
    IF document \in StagedDocuments
    THEN \E index \in DOMAIN prep : prep[index].doc = document /\ prep[index].effect = "write"
    ELSE Majority(ptrLive[document]) \in Vers
RemovalIsBelowWrite(path) ==
    \E index \in DOMAIN prep :
        /\ prep[index].effect = "write"
        /\ (PathContains(path, DocumentEntry(prep[index].doc))
            \/ PathContains(DocumentEntry(prep[index].doc), path))
RemovalCrossesDocument(path) ==
    \E document \in Docs :
        Majority(ptrLive[document]) \in Vers
        /\ PathContains(DocumentEntry(document), path)
NodeRemovalPreconditionsHold ==
    \A path \in PlannedRemovalTargets :
        CASE path = NamespaceRoot ->
                 \A document \in Docs : ~DocumentAfterTransaction(document)
          [] OTHER -> ~RemovalCrossesDocument(path)
SwapSetPreconditionsHold ==
    /\ SwapSetHasUniqueTargets
    /\ ~\E path \in PlannedRemovalTargets : RemovalIsBelowWrite(path)
    /\ NodeRemovalPreconditionsHold

SwapPreconditionHolds(swap) ==
    CASE swap.precondition = "unconditional" -> TRUE
      [] swap.precondition = "digest" -> Majority(ptrLive[swap.doc]) = swap.expected
      [] OTHER -> Majority(ptrLive[swap.doc]) \notin Vers
ExpectedMismatch(p, d) ==
    LET held == Majority(ptrLive[d])
    IN IF V(p) # held THEN V(p) ELSE V(p) + 1
PreconditionsHold ==
    \A index \in DOMAIN prep : SwapPreconditionHolds(prep[index])

PreparationTypeOK ==
    /\ ptrLive \in [Docs -> [Ptr -> Vers \cup {NoVal}]]
    /\ ptrDur \in [Docs -> [Ptr -> Vers \cup {NoVal}]]
    /\ prep \in Seq(PreparedSwaps)

RemovalTypeOK ==
    /\ removeNodes \in Seq(NamespaceNodes)
    /\ Cardinality({path : path \in {removeNodes[index] : index \in DOMAIN removeNodes}})
           = Len(removeNodes)

NodeTypeOK == nodeLive \in [Docs -> [Ptr -> BOOLEAN]] /\ nodeDur \in [Docs -> [Ptr -> BOOLEAN]]

NamespacePhaseTypeOK == wph \in Phases
NamespaceDocumentTypeOK == wdoc \in Docs \cup {0}
NamespaceReplicaTypeOK == wix \in Ptr \cup Blob
NamespaceIndexTypeOK == txnDoc \in 0..(Cardinality(Docs) + Len(removeNodes))
NamespaceWorkTypeOK == NamespacePhaseTypeOK /\ NamespaceDocumentTypeOK
    /\ NamespaceReplicaTypeOK /\ NamespaceIndexTypeOK

DirectoryTypeOK ==
    /\ directoryLive \in 0..(InitialDirectoryDebt + 1)
    /\ directoryDurable \in 0..(InitialDirectoryDebt + 1)
    /\ directoryTorn \in BOOLEAN

NamespaceTypeOK == PreparationTypeOK /\ RemovalTypeOK /\ NodeTypeOK
    /\ NamespaceWorkTypeOK /\ DirectoryTypeOK

\* Audience: internal.
SpTypeOK ==
    /\ TypeOK
    /\ ptrLive \in [Docs -> [Ptr -> Vers \cup {NoVal}]]
    /\ ptrDur \in [Docs -> [Ptr -> Vers \cup {NoVal}]]
    /\ blobLive \in [Docs -> [Blob -> SUBSET Vers]]
    /\ blobDur \in [Docs -> [Blob -> SUBSET Vers]]
    /\ record \in [Docs -> [live : Vers \cup {NoVal}, dur : Vers \cup {NoVal}]]
    /\ prep \in Seq(PreparedSwaps)
    /\ removeNodes \in Seq(NamespaceNodes)
    /\ Cardinality({path : path \in {removeNodes[index] : index \in DOMAIN removeNodes}})
           = Len(removeNodes)
    /\ nodeLive \in [Docs -> [Ptr -> BOOLEAN]]
    /\ nodeDur \in [Docs -> [Ptr -> BOOLEAN]]
    /\ wph \in Phases
    /\ wdoc \in Docs \cup {0}
    /\ wix \in Ptr \cup Blob
    /\ wkeep \in SUBSET Vers
    /\ txnDoc \in 0..Cardinality(Docs) + Len(removeNodes)
    /\ runPhase \in [Procs -> RunPhases]
    /\ runCount \in [Procs -> (0..MaxCount) \cup {NoVal}]
    /\ unitReady \in [Procs -> BOOLEAN]
    /\ requestRecovery \in [Procs -> BOOLEAN]
    /\ directoryLive \in 0..InitialDirectoryDebt + 1
    /\ directoryDurable \in 0..InitialDirectoryDebt + 1
    /\ directoryTorn \in BOOLEAN
    /\ runDirectories \in [Procs -> 0..InitialDirectoryDebt + 1]
    /\ unitIntervened \in [Procs -> BOOLEAN]
    /\ writes \in 0..MaxRequests
    /\ ordinaryRequests \in 0..MaxRequests
    /\ rkinds \in SUBSET {"unlogged", "logged"}

SpInit ==
    /\ Init
    /\ ptrLive = [d \in Docs |-> [r \in Ptr |-> IF InitialNodeRemoved THEN NoVal ELSE 0]]
    /\ ptrDur = ptrLive
    /\ blobLive = [d \in Docs |-> [k \in Blob |-> IF InitialNodeRemoved THEN {} ELSE {0}]]
    /\ blobDur = blobLive
    /\ record = [d \in Docs |-> NoRecord]
    /\ prep = NoPrep /\ removeNodes = NoPrep
    /\ nodeLive = [d \in Docs |-> [r \in Ptr |-> ~InitialNodeRemoved]]
    /\ nodeDur = nodeLive
    /\ wph = "none" /\ wdoc = 0 /\ wix = 1 /\ wkeep = {}
    /\ txnDoc = 0
    /\ runPhase = [holder \in Procs |-> "none"]
    /\ runCount = [holder \in Procs |-> NoVal]
    /\ unitReady = [holder \in Procs |-> FALSE]
    /\ requestRecovery = [holder \in Procs |-> FALSE]
    /\ directoryLive = 0 /\ directoryDurable = InitialDirectoryDebt
    /\ directoryTorn = FALSE
    /\ runDirectories = [holder \in Procs |-> 0]
    /\ unitIntervened = [holder \in Procs |-> FALSE]
    /\ writes = 0
    /\ ordinaryRequests = 0
    /\ rkinds = {}

EffectivePhase(holder) ==
    IF runPhase[holder] = "none" THEN "none"
        ELSE IF count # runCount[holder] /\ PubControl # "resume-stale-recovery"
            THEN IF directoryLive > 0 THEN "directories" ELSE "publication"
        ELSE IF runPhase[holder] = "directories" /\ directoryLive = 0 /\ unitIntervened[holder] THEN "publication"
    ELSE IF log = "empty" /\ runPhase[holder] \in {"transaction", "collection"} THEN "claims"
    ELSE runPhase[holder]

RunUnitFinished(holder) ==
    /\ pc[holder] \in RecStates
    /\ \/ wph = "unitStart"
         \/ (runPhase[holder] = "directories" /\ wph = "dirDone")
       \/ (runPhase[holder] = "publication" /\ wph = "rvDone")
       \/ (runPhase[holder] = "transaction" /\ wph = "unitTxnDone")
       \/ (runPhase[holder] = "collection" /\ wph = "txDone")
       \/ (runPhase[holder] = "claims" /\ wph = "claimsDone")

SRunStart(holder) ==
    /\ W(holder) /\ wph = "none"
    /\ pc[holder] \in {"held", "told"}
    /\ requestRecovery[holder] /\ unitReady[holder]
    /\ IF runPhase[holder] = "none" THEN pc[holder] = "told" ELSE unitReady[holder]
    /\ LET phase == EffectivePhase(holder)
       IN /\ pc' = Set(pc, holder,
                                 CASE phase = "publication" ->
                                                IF ~Txn /\ Partial /\ log # "prepared" THEN "recWhole" ELSE "recEnd"
                                     [] phase \in {"none", "directories", "claims"} -> "recEnd"
                   [] phase = "transaction" ->
                        IF log = "prepared" /\ Control # "recovery-rolls-back-a-prepared-transaction"
                        THEN "recSwap1" ELSE "recEnd"
                   [] OTHER -> "recReset")
          /\ wph' = CASE phase = "none" -> "unitStart"
                  [] phase = "directories" ->
                      IF (IF count # runCount[holder] \/ unitIntervened[holder] THEN directoryLive ELSE runDirectories[holder]) > 0
                      THEN "dirSync" ELSE "dirClear"
                      [] phase = "claims" -> "claimsDone"
                      [] phase = "completion" -> "txDone"
                      [] phase = "collection" -> IF prep = NoPrep THEN "txDone" ELSE "rvCollRead"
                      [] OTHER -> "none"
          /\ txnDoc' = IF phase = "collection" /\ prep # NoPrep THEN 1 ELSE txnDoc
          /\ wdoc' = IF phase = "collection" /\ prep # NoPrep THEN prep[1].doc ELSE 0
    /\ wix' = 1 /\ wkeep' = {}
    /\ fence' = IF Control = "fence-cleared-at-recovery-start" THEN FALSE ELSE fence
    /\ UNCHANGED <<adm, req, lockHolder, owner, trusted, count, gen,
                   store, handle, reader, terminatedIn, counts, docs, prep>>

RunUnitEnd(holder) ==
    /\ W(holder) /\ RunUnitFinished(holder)
    /\ pc' = Set(pc, holder, "told")
    /\ NoWork
    /\ UNCHANGED <<adm, req, coord, store, handle, reader, terminatedIn, counts, docs>>

DirectorySync(holder) ==
    /\ W(holder) /\ wph = "dirSync"
    /\ wph' = "dirDone"
    /\ UNCHANGED <<vars, docs, wdoc, wix, wkeep, prep, txnDoc>>

DirectoryClear(holder) ==
    /\ W(holder) /\ wph = "dirClear"
    /\ wph' = "dirClearSync"
    /\ UNCHANGED <<vars, docs, wdoc, wix, wkeep, prep, txnDoc>>

DirectoryClearSync(holder) ==
    /\ W(holder) /\ wph = "dirClearSync"
    /\ wph' = "dirDone"
    /\ UNCHANGED <<vars, docs, wdoc, wix, wkeep, prep, txnDoc>>

-----------------------------------------------------------------------------
(* The contract's steps that touch no document.                            *)

Quiet(A) == A /\ UNCHANGED <<docs, work, txnDoc>>

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
    /\ UNCHANGED <<work, txnDoc>>

-----------------------------------------------------------------------------
(* The contract's steps that end or fail the work.                         *)

\* An operation that ends before its first effect.
SEndEmpty(p) ==
    /\ EndEmpty(p)
    /\ wph' = "done" /\ wdoc' = 0 /\ wix' = 1 /\ wkeep' = {}
    /\ UNCHANGED <<docs, txnDoc>>

SAbort(p) ==
    /\ Abort(p)
    /\ wph = "rbDone"
    /\ prep' = NoPrep /\ txnDoc' = 0
    /\ wph' = "done" /\ wdoc' = 0 /\ wix' = 1 /\ wkeep' = {}
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, record>>

SFailUnresolved(p) == FailUnresolved(p) /\ NoWork /\ UNCHANGED docs

\* The closing raise begins once the work, collection included, is over.
SBeginClose(p) ==
    /\ BeginClose(p) /\ wph = "done"
    /\ prep' = NoPrep /\ txnDoc' = 0
    /\ wph' = "none" /\ wdoc' = 0 /\ wix' = 1 /\ wkeep' = {}
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, record>>

\* The prepare record names every staged document and version; one whose append
\* failed may or may not be in the log, and the record says the same as the log.
ApplyTargetPhase(index, replica) ==
    IF prep[index].effect = "write" /\ ~nodeLive[prep[index].doc][replica]
    THEN IF directoryTorn THEN "dirAppendRepair" ELSE "dirAppend"
    ELSE "txRen"

SPrepare(p) ==
    /\ Prepare(p)
    /\ wph = "staged" /\ (prep # NoPrep \/ removeNodes # NoPrep)
    /\ wph' = IF prep = NoPrep THEN "nodeRen" ELSE ApplyTargetPhase(1, 1)
    /\ txnDoc' = 1
    /\ wdoc' = IF prep = NoPrep THEN 0 ELSE prep[1].doc
    /\ wix' = 1
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, record, prep, wkeep>>

SPrepareFails(p) ==
    /\ PrepareFails(p)
    /\ wph = "staged"
    /\ NoWork
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, record, prep>>

\* Phase 1 accepts every transaction precondition before Begin writes a log record.
IngestPhase(index) == IF prep[index].effect = "write" THEN "ingRen" ELSE "skipIng"

TransactionPreconditionsHold == PreconditionsHold /\ SwapSetPreconditionsHold

SBegin(p) ==
    /\ Begin(p) /\ wph = "planned" /\ TransactionPreconditionsHold
    /\ txnDoc' = IF prep = NoPrep THEN 0 ELSE 1
    /\ wdoc' = IF prep = NoPrep THEN 0 ELSE prep[1].doc
    /\ wix' = 1 /\ wph' = IF prep = NoPrep THEN "staged" ELSE IngestPhase(1)
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, record, prep, wkeep>>

SReject(p) ==
    /\ EndEmpty(p) /\ wph = "planned" /\ ~TransactionPreconditionsHold
    /\ prep' = NoPrep /\ txnDoc' = 0
    /\ wph' = "done" /\ wdoc' = 0 /\ wix' = 1 /\ wkeep' = {}
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, record>>

SIngest(p) == Ingest(p) /\ wph = "staged" /\ UNCHANGED <<docs, work, txnDoc>>

SEnd(p) == End(p) /\ UNCHANGED <<docs, work, txnDoc>>

SRecSwap1(p) == RecSwap1(p) /\ wph = "txDone" /\ NoWork /\ UNCHANGED docs

SRecEnd(p) ==
    /\ RecEnd(p)
    /\ runPhase[p] = "transaction" /\ wph = "none"
    /\ NoWork
    /\ UNCHANGED docs

SRecStop(p) == RecStop(p) /\ NoWork /\ UNCHANGED docs

SRecSweep(holder) ==
    /\ RecSweep(holder)
    /\ runPhase[holder] = "transaction"
    /\ wph' = "unitTxnDone"
    /\ txnDoc' = 0 /\ wdoc' = 0 /\ wix' = 1
    /\ UNCHANGED <<docs, wkeep>>

SRecReset(holder) ==
    /\ RecReset(holder) /\ wph = "txDone"
    /\ runPhase[holder] = "completion"
    /\ prep' = NoPrep /\ txnDoc' = 0
    /\ wph' = "none" /\ wdoc' = 0 /\ wix' = 1 /\ wkeep' = {}
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, record>>

\* EXTERNAL: a process dies. The documents stay as the live filesystem has them,
\* and its work in progress is gone with it.
STerminate(p) ==
    /\ Terminate(p)
    /\ IF lockHolder = p
       THEN /\ prep' = IF log = "prepared" THEN prep ELSE NoPrep
            /\ wph' = "none" /\ wdoc' = 0 /\ wix' = 1 /\ wkeep' = {} /\ txnDoc' = 0
       ELSE UNCHANGED <<work, prep, txnDoc>>
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, record>>

\* EXTERNAL: power loss. Every document reverts to what was durable.
SCrash ==
    /\ Crash
    /\ ptrLive' = ptrDur
    /\ blobLive' = blobDur
    /\ record' = [d \in Docs |-> [live |-> record[d].dur, dur |-> record[d].dur]]
    /\ prep' = IF log = "prepared" THEN prep ELSE NoPrep
    /\ wph' = "none" /\ wdoc' = 0 /\ wix' = 1 /\ wkeep' = {} /\ txnDoc' = 0
    /\ UNCHANGED <<ptrDur, blobDur>>

-----------------------------------------------------------------------------
(* A write's work on its document.                                         *)

PlanTxn(p, d, precondition, matches, effect) ==
    /\ W(p) /\ Txn /\ pc[p] = "open" /\ wph \in {"none", "planned"}
    /\ d \notin StagedDocuments
    /\ prep' = Append(prep, [doc |-> d, ver |-> V(p), effect |-> effect,
                           precondition |-> precondition,
                           expected |-> CASE precondition = "unconditional" -> NoVal
                                        [] precondition = "digest" ->
                                             IF matches THEN Majority(ptrLive[d]) ELSE ExpectedMismatch(p, d)
                                        [] OTHER -> NoVal])
    /\ (precondition # "absent" \/ ~matches)
    /\ wph' = "planned" /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, blobDur, record,
                                       removeNodes, nodeLive, nodeDur,
                                       wdoc, wix, wkeep, txnDoc>>

PlanSwapCollision(p, d) ==
    /\ W(p) /\ Txn /\ pc[p] = "open" /\ wph = "planned"
    /\ SwapSetHasUniqueTargets
    /\ d \in StagedDocuments
    /\ prep' = Append(prep, [doc |-> d, ver |-> V(p), effect |-> "remove",
                           precondition |-> "unconditional", expected |-> NoVal])
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, blobDur, record,
                   removeNodes, nodeLive, nodeDur, wph, wdoc, wix, wkeep, txnDoc>>

PlannedPath(marker) ==
    CASE marker = "planRemove" -> NamespaceRoot
      [] marker = "planRemoveAtDocument" -> DocumentEntry(1)
      [] OTHER -> DocumentBelow(1)

PlanNodeRemoval(p, marker) ==
    /\ W(p) /\ Txn /\ pc[p] = "open" /\ wph \in {"none", "planned"}
    /\ marker \in {"planRemove", "planRemoveAtDocument", "planRemoveBelowDocument"}
    /\ PlannedPath(marker) \notin PlannedRemovalTargets
    /\ wph' = marker
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, blobDur, record, prep,
                   nodeLive, nodeDur, wdoc, wix, wkeep, txnDoc>>

FinishNodePlan(p) ==
    /\ W(p) /\ wph \in {"planRemove", "planRemoveAtDocument", "planRemoveBelowDocument"}
    /\ wph' = "planned" /\ wdoc' = 0
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, blobDur, record, prep,
                   removeNodes, nodeLive, nodeDur, wix, wkeep, txnDoc>>

Pick(p, d) ==
    /\ W(p) /\ ~Txn /\ pc[p] = "open" /\ wph = "none"
    /\ d \notin StagedDocuments
    /\ wdoc' = d /\ wph' = "ingRen" /\ wix' = 1
    /\ UNCHANGED <<vars, docs, wkeep, txnDoc>>

IngRen(p) ==
    /\ W(p) /\ wph = "ingRen"
    /\ blobLive' = [blobLive EXCEPT ![wdoc][wix] = @ \cup {V(p)}]
    /\ wph' = "ingSync"
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobDur, record, prep, wdoc, wix, wkeep,
                   prep, txnDoc>>

\* A transaction's write ingests while it is begun.
AfterIngest(p) ==
    IF pc[p] = "begun" THEN "staged" ELSE IF Fix = "record" THEN "recWrite" ELSE "ptrRen"

SkipIng(p) ==
    /\ W(p) /\ wph = "skipIng"
    /\ IF txnDoc < Len(prep)
       THEN txnDoc' = txnDoc + 1 /\ wdoc' = prep[txnDoc + 1].doc
            /\ wix' = 1 /\ wph' = IngestPhase(txnDoc + 1)
       ELSE wph' = "staged" /\ UNCHANGED <<txnDoc, wdoc, wix>>
    /\ UNCHANGED <<vars, docs, wkeep>>

IngSync(p) ==
    /\ W(p) /\ wph = "ingSync"
    /\ blobDur' = [blobDur EXCEPT ![wdoc][wix] = blobLive[wdoc][wix]]
    /\ IF wix < NBlob
          THEN wix' = wix + 1 /\ wph' = "ingRen"
               /\ UNCHANGED <<prep, txnDoc, wdoc>>
          ELSE IF Txn /\ txnDoc < Len(prep)
               THEN txnDoc' = txnDoc + 1 /\ wdoc' = prep[txnDoc + 1].doc
                    /\ wix' = 1 /\ wph' = IngestPhase(txnDoc + 1) /\ UNCHANGED prep
               ELSE wix' = 1 /\ wph' = AfterIngest(p)
                    /\ IF Txn
                       THEN UNCHANGED <<prep, txnDoc, wdoc>>
                       ELSE prep' = Append(prep, [doc |-> wdoc, ver |-> V(p),
                                                   effect |-> "write",
                                                   precondition |-> "unconditional",
                                                   expected |-> NoVal])
                            /\ txnDoc' = 0 /\ UNCHANGED wdoc
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, record, wkeep>>

\* "record": the append decides the outcome, so it is the first effect.
RecWrite(p) ==
    /\ W(p) /\ wph = "recWrite"
    /\ FirstEffect(p)
    /\ record' = [record EXCEPT ![wdoc].live = V(p)]
    /\ wph' = "recSync"
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, prep, wdoc, wix, wkeep,
                   prep, txnDoc>>

RecSync(p) ==
    /\ W(p) /\ wph = "recSync"
    /\ record' = [record EXCEPT ![wdoc].dur = record[wdoc].live]
    /\ wph' = "ptrRen"
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, blobDur, prep, wdoc, wix, wkeep,
                   prep, txnDoc>>

\* The first rename is the first effect when nothing before it was.
PtrRen(p) ==
    /\ W(p) /\ wph = "ptrRen"
    /\ IF pc[p] \in {"open", "prepared"} THEN FirstEffect(p) ELSE UNCHANGED vars
    /\ ptrLive' = [ptrLive EXCEPT ![wdoc][wix] = V(p)]
    /\ wph' = "ptrSync"
    /\ UNCHANGED <<ptrDur, blobLive, blobDur, record, prep, wdoc, wix, wkeep,
                   prep, txnDoc>>

PtrSync(p) ==
    /\ W(p) /\ wph = "ptrSync"
    /\ ptrDur' = [ptrDur EXCEPT ![wdoc][wix] = ptrLive[wdoc][wix]]
    /\ IF wix < NPtr
          THEN wix' = wix + 1 /\ wph' = "ptrRen" /\ UNCHANGED vars
       ELSE IF Fix = "record" /\ ~TxnOp
          THEN wix' = 1 /\ wph' = "recClr" /\ UNCHANGED vars
          ELSE wix' = 1 /\ wph' = "collRead" /\ LastEffect(p)
    /\ UNCHANGED <<ptrLive, blobLive, blobDur, record, prep, wdoc, wkeep,
                   prep, txnDoc>>

RecClr(p) ==
    /\ W(p) /\ wph = "recClr"
    /\ record' = [record EXCEPT ![wdoc].live = NoVal]
    /\ wph' = "recClrSync"
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, blobDur, prep, wdoc, wix, wkeep,
                   prep, txnDoc>>

\* The clear is durable: the write's work is complete.
RecClrSync(p) ==
    /\ W(p) /\ wph = "recClrSync"
    /\ LastEffect(p)
    /\ record' = [record EXCEPT ![wdoc].dur = NoVal]
    /\ wph' = "collRead"
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, prep, wdoc, wix, wkeep,
                   prep, txnDoc>>

-----------------------------------------------------------------------------
(* Collection of what the write superseded, once its operation is done.    *)

CollNext ==
    IF txnDoc < Len(prep)
    THEN txnDoc' = txnDoc + 1 /\ wdoc' = prep[txnDoc + 1].doc
         /\ wix' = 1 /\ wph' = "collRead"
    ELSE txnDoc' = 0 /\ wdoc' = 0 /\ wix' = 1 /\ wph' = "done"

CollRead(p) ==
    /\ W(p) /\ pc[p] = "wrote" /\ wph = "collRead"
    /\ LET m == Majority(ptrLive[wdoc])
       IN  /\ wkeep' = IF m \in Vers THEN {m} ELSE {}
           /\ wph' = "collSync" /\ wix' = 1
           /\ UNCHANGED <<wdoc, txnDoc>>
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, blobDur, record, prep,
                   removeNodes, nodeLive, nodeDur>>
CollSync(p) ==
    /\ W(p) /\ wph = "collSync"
    /\ ptrDur' = [ptrDur EXCEPT ![wdoc][wix] = ptrLive[wdoc][wix]]
    /\ IF wix < NPtr
          THEN wix' = wix + 1 /\ wph' = "collSync"
          ELSE wix' = 1 /\ wph' = "collRem"
    /\ UNCHANGED <<vars, ptrLive, blobLive, blobDur, record, prep, wdoc, wkeep, txnDoc>>

CollReplicaNext ==
    IF wix < NBlob
    THEN wix' = wix + 1 /\ wph' = "collRem"
         /\ UNCHANGED <<txnDoc, wdoc>>
    ELSE CollNext

CollRem(p) ==
    /\ W(p) /\ wph = "collRem"
    /\ IF blobLive[wdoc][wix] \subseteq wkeep
          THEN CollReplicaNext /\ UNCHANGED blobLive
          ELSE /\ blobLive' = [blobLive EXCEPT ![wdoc][wix] = @ \cap wkeep]
               /\ wph' = "collFsync"
               /\ UNCHANGED <<wix, txnDoc, wdoc>>
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobDur, record, prep, wkeep>>

CollFsync(p) ==
    /\ W(p) /\ wph = "collFsync"
    /\ blobDur' = [blobDur EXCEPT ![wdoc][wix] = blobLive[wdoc][wix]]
    /\ CollReplicaNext
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, record, prep, wkeep>>

\* A collection that fails leaves the rest of the garbage behind.
CollFails(p) ==
    /\ W(p) /\ wph \in {"collRead", "collSync", "collRem", "collFsync"}
    /\ pc[p] = "wrote"
    /\ wph' = "done"
    /\ UNCHANGED <<vars, docs, wdoc, wix, wkeep, prep, txnDoc>>

-----------------------------------------------------------------------------
(* Recovery's work on the documents, before it ends the log.               *)

FirstRecoveryPhase ==
    IF PubControl = "recovery-leaves-the-documents" THEN "rvDone"
    ELSE CASE Fix = "record"  -> "rvRecSync"
           [] Fix = "resolve" -> "rsRen"
           [] OTHER           -> "rvDone"

RvBegin(p) ==
    /\ W(p) /\ pc[p] = "recEnd" /\ wph = "none"
    /\ runPhase[p] = "publication"
    /\ wph' = FirstRecoveryPhase /\ wdoc' = 1 /\ wix' = 1
    /\ UNCHANGED <<vars, docs, wkeep, prep, txnDoc>>

\* The document after this one, or the end of recovery's work on them.
NextDoc ==
    IF wdoc < (IF PubControl = "recovery-skips-a-document" THEN 1 ELSE LastDoc)
    THEN wdoc' = wdoc + 1 /\ wix' = 1 /\ wph' = FirstRecoveryPhase
         /\ UNCHANGED <<prep, txnDoc>>
    ELSE wdoc' = 0 /\ wix' = 1 /\ wph' = "rvDone"
         /\ UNCHANGED <<prep, txnDoc>>

\* "record": the record file is synced first, so a failed clear is seen, then
\* what it names is rolled forward and the record cleared.
RvRecSync(p) ==
    /\ W(p) /\ wph = "rvRecSync"
    /\ record' = [record EXCEPT ![wdoc].dur = record[wdoc].live]
    /\ IF record[wdoc].live # NoVal
          THEN wph' = "rvRen" /\ wix' = 1
               /\ UNCHANGED <<wdoc, prep, txnDoc>>
          ELSE NextDoc
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, blobDur, prep, wkeep>>

RvRen(p) ==
    /\ W(p) /\ wph = "rvRen"
    /\ ptrLive' = [ptrLive EXCEPT ![wdoc][wix] = record[wdoc].live]
    /\ wph' = "rvSync"
    /\ UNCHANGED <<vars, ptrDur, blobLive, blobDur, record, prep, wdoc, wix, wkeep,
                   prep, txnDoc>>

RvSync(p) ==
    /\ W(p) /\ wph = "rvSync"
    /\ ptrDur' = [ptrDur EXCEPT ![wdoc][wix] = ptrLive[wdoc][wix]]
    /\ IF wix < NPtr
          THEN wix' = wix + 1 /\ wph' = "rvRen"
          ELSE wix' = 1 /\ wph' = "rvClr"
    /\ UNCHANGED <<vars, ptrLive, blobLive, blobDur, record, prep, wdoc, wkeep,
                   prep, txnDoc>>

RvClr(p) ==
    /\ W(p) /\ wph = "rvClr"
    /\ record' = [record EXCEPT ![wdoc].live = NoVal]
    /\ wph' = "rvClrSync"
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, blobDur, prep, wdoc, wix, wkeep,
                   prep, txnDoc>>

RvClrSync(p) ==
    /\ W(p) /\ wph = "rvClrSync"
    /\ record' = [record EXCEPT ![wdoc].dur = NoVal]
    /\ wph' = "rvCollRead" /\ UNCHANGED <<wdoc, wix, prep, txnDoc>>
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, blobDur, prep, wkeep>>

RvCollected ==
    IF runPhase[lockHolder] = "collection"
    THEN IF txnDoc < Len(prep)
         THEN wph' = "rvCollRead" /\ txnDoc' = txnDoc + 1
              /\ wdoc' = prep[txnDoc + 1].doc /\ wix' = 1 /\ UNCHANGED prep
         ELSE wph' = "txDone" /\ txnDoc' = 0 /\ wdoc' = 0 /\ wix' = 1
              /\ UNCHANGED prep
    ELSE NextDoc

\* What the roll-forward superseded is collected as a write's collection is: the
\* entry is synced on every replica, then every other version removed.
RvCollRead(p) ==
    /\ W(p) /\ wph = "rvCollRead"
    /\ LET m == Majority(ptrLive[wdoc])
       IN  IF m \in Vers
           THEN wkeep' = {m} /\ wph' = "rvCollSync" /\ wix' = 1
                /\ UNCHANGED <<wdoc, prep, txnDoc>>
            ELSE RvCollected /\ UNCHANGED wkeep
    /\ UNCHANGED <<vars, docs>>

RvCollSync(p) ==
    /\ W(p) /\ wph = "rvCollSync"
    /\ ptrDur' = [ptrDur EXCEPT ![wdoc][wix] = ptrLive[wdoc][wix]]
    /\ IF wix < NPtr
          THEN wix' = wix + 1 /\ wph' = "rvCollSync"
          ELSE wix' = 1 /\ wph' = "rvCollRem"
    /\ UNCHANGED <<vars, ptrLive, blobLive, blobDur, record, prep, wdoc, wkeep,
                   prep, txnDoc>>

RvCollNext ==
    IF wix < NBlob
    THEN wix' = wix + 1 /\ wph' = "rvCollRem"
         /\ UNCHANGED <<wdoc, prep, txnDoc>>
    ELSE RvCollected

RvCollRem(p) ==
    /\ W(p) /\ wph = "rvCollRem"
    /\ IF blobLive[wdoc][wix] \subseteq wkeep
          THEN RvCollNext /\ UNCHANGED blobLive
          ELSE /\ blobLive' = [blobLive EXCEPT ![wdoc][wix] = @ \cap wkeep]
               /\ wph' = "rvCollFsync"
               /\ UNCHANGED <<wix, wdoc, txnDoc>>
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobDur, record, prep, wkeep>>

RvCollFsync(p) ==
    /\ W(p) /\ wph = "rvCollFsync"
    /\ blobDur' = [blobDur EXCEPT ![wdoc][wix] = blobLive[wdoc][wix]]
    /\ RvCollNext
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, record, prep, wkeep>>

RbBegin(holder) ==
    /\ W(holder) /\ pc[holder] \in {"begun", "ingested"}
    /\ wph \in {"none", "ingRen", "ingSync", "staged"}
    /\ stage' = 0
    /\ pc' = Set(pc, holder, "begun")
    /\ IF prep = NoPrep
          THEN wph' = "rbDone" /\ txnDoc' = 0 /\ wdoc' = 0
          ELSE wph' = "rbCollRead" /\ txnDoc' = 1 /\ wdoc' = prep[1].doc
    /\ wix' = 1
    /\ UNCHANGED <<adm, req, coord, a, b, shared, log, ptgt,
                   handle, reader, terminatedIn, counts, docs, wkeep, prep>>

RbNext ==
    IF txnDoc < Len(prep)
    THEN wph' = "rbCollRead" /\ txnDoc' = txnDoc + 1
         /\ wdoc' = prep[txnDoc + 1].doc /\ wix' = 1
    ELSE /\ wph' = (IF pc[lockHolder] = "recEnd" THEN "rvDone" ELSE "rbDone")
         /\ txnDoc' = 0 /\ wdoc' = 0 /\ wix' = 1

RbCollected == RbNext

RbCollRead(holder) ==
    /\ W(holder) /\ wph = "rbCollRead"
    /\ LET majority == Majority(ptrLive[wdoc])
       IN IF majority \in Vers
          THEN wkeep' = {majority} /\ wph' = "rbCollSync" /\ wix' = 1
               /\ UNCHANGED <<wdoc, prep, txnDoc>>
          ELSE RbCollected /\ UNCHANGED wkeep
    /\ UNCHANGED <<vars, docs>>

RbCollSync(holder) ==
    /\ W(holder) /\ wph = "rbCollSync"
    /\ ptrDur' = [ptrDur EXCEPT ![wdoc][wix] = ptrLive[wdoc][wix]]
    /\ IF wix < NPtr
          THEN wix' = wix + 1 /\ wph' = "rbCollSync"
          ELSE wix' = 1 /\ wph' = "rbCollRem"
    /\ UNCHANGED <<vars, ptrLive, blobLive, blobDur, record, prep, wdoc, wkeep,
                   prep, txnDoc>>

RbCollNext ==
    IF wix < NBlob
    THEN wix' = wix + 1 /\ wph' = "rbCollRem"
         /\ UNCHANGED <<txnDoc, wdoc>>
    ELSE RbNext

RbCollRem(holder) ==
    /\ W(holder) /\ wph = "rbCollRem"
    /\ IF blobLive[wdoc][wix] \subseteq wkeep
          THEN RbCollNext /\ UNCHANGED blobLive
          ELSE /\ blobLive' = [blobLive EXCEPT ![wdoc][wix] = @ \cap wkeep]
               /\ wph' = "rbCollFsync"
               /\ UNCHANGED <<wix, txnDoc, wdoc>>
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobDur, record, prep, wkeep>>

RbCollFsync(holder) ==
    /\ W(holder) /\ wph = "rbCollFsync"
    /\ blobDur' = [blobDur EXCEPT ![wdoc][wix] = blobLive[wdoc][wix]]
    /\ RbCollNext
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, record, prep, wkeep>>

RbCollFails(holder) ==
    /\ W(holder) /\ pc[holder] \in {"begun", "ingested"}
    /\ wph \in {"rbCollRead", "rbCollSync", "rbCollRem", "rbCollFsync"}
    /\ pc' = Set(pc, holder, "told") /\ fence' = TRUE
    /\ NoWork
    /\ UNCHANGED <<adm, req, lockHolder, owner, trusted, count, gen,
                   store, handle, reader, terminatedIn, counts, docs>>

\* "resolve": every replica is set to the document's majority and synced.
RsRen(p) ==
    /\ W(p) /\ wph = "rsRen"
    /\ LET m == Majority(ptrLive[wdoc])
       IN  IF m \in Vers
           THEN ptrLive' = [ptrLive EXCEPT ![wdoc][wix] = m] /\ wph' = "rsSync"
                /\ UNCHANGED <<wdoc, wix, prep, txnDoc>>
           ELSE UNCHANGED ptrLive /\ NextDoc
    /\ UNCHANGED <<vars, ptrDur, blobLive, blobDur, record, prep, wkeep>>

RsSync(p) ==
    /\ W(p) /\ wph = "rsSync"
    /\ ptrDur' = [ptrDur EXCEPT ![wdoc][wix] = ptrLive[wdoc][wix]]
    /\ IF wix < NPtr
          THEN wix' = wix + 1 /\ wph' = "rsRen"
               /\ UNCHANGED <<wdoc, prep, txnDoc>>
          ELSE NextDoc
    /\ UNCHANGED <<vars, ptrLive, blobLive, blobDur, record, prep, wkeep>>

\* A prepared transaction is rolled forward from its prepare record while the
\* contract makes its swaps.
TxBegin(p) ==
    /\ W(p) /\ pc[p] = "recSwap1" /\ wph = "none"
    /\ (prep # NoPrep \/ removeNodes # NoPrep)
    /\ txnDoc' = 1
    /\ wph' = IF prep = NoPrep THEN "nodeRen" ELSE ApplyTargetPhase(1, 1)
    /\ wdoc' = IF prep = NoPrep THEN 0 ELSE prep[1].doc
    /\ wix' = 1
    /\ UNCHANGED <<vars, docs, wkeep>>

TxRen(p) ==
    /\ W(p) /\ wph = "txRen"
    /\ ptrLive' = [ptrLive EXCEPT ![wdoc][wix] =
                       IF prep[txnDoc].effect = "write" THEN prep[txnDoc].ver ELSE NoVal]
    /\ wph' = "txSync"
    /\ UNCHANGED <<vars, ptrDur, blobLive, blobDur, record, prep, wdoc, wix, wkeep,
                   prep, txnDoc>>

TxSync(p) ==
    /\ W(p) /\ wph = "txSync"
    /\ ptrDur' = [ptrDur EXCEPT ![wdoc][wix] = ptrLive[wdoc][wix]]
    /\ IF wix < NPtr
          THEN wix' = wix + 1 /\ wph' = ApplyTargetPhase(txnDoc, wix + 1)
               /\ UNCHANGED <<txnDoc, wdoc>> /\ UNCHANGED vars
          ELSE IF txnDoc < Len(prep)
               THEN txnDoc' = txnDoc + 1 /\ wdoc' = prep[txnDoc + 1].doc
                    /\ wix' = 1 /\ wph' = ApplyTargetPhase(txnDoc + 1, 1) /\ UNCHANGED vars
               ELSE IF removeNodes # NoPrep
                    THEN wph' = "nodeRen" /\ txnDoc' = 1 /\ wdoc' = 0 /\ wix' = 1
                         /\ UNCHANGED vars
                    ELSE IF pc[p] = "prepared"
                         THEN /\ LastEffect(p)
                              /\ txnDoc' = 1 /\ wdoc' = prep[1].doc /\ wix' = 1 /\ wph' = "collRead"
                         ELSE /\ wph' = "txDone" /\ txnDoc' = 1 /\ wdoc' = prep[1].doc
                              /\ wix' = 1 /\ UNCHANGED vars
    /\ UNCHANGED <<ptrLive, blobLive, blobDur, record, prep, wkeep>>

DirectoryIntentAppend(p) ==
    /\ W(p) /\ wph \in {"dirAppend", "dirAppendRepair"}
    /\ wph' = "dirAppendSync"
    /\ UNCHANGED <<vars, docs, wdoc, wix, wkeep, txnDoc>>

DirectoryIntentTorn(p) ==
    /\ FailUnresolved(p) /\ wph = "dirAppend"
    /\ NoWork
    /\ UNCHANGED docs

DirectoryIntentSync(p) ==
    /\ W(p) /\ wph = "dirAppendSync"
    /\ wph' = "dirCreate"
    /\ UNCHANGED <<vars, docs, wdoc, wix, wkeep, txnDoc>>

DirectoryCreate(p) ==
    /\ W(p) /\ wph = "dirCreate"
    /\ wph' = "dirRename"
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, blobDur, record, prep,
                   removeNodes, nodeDur, wdoc, wix, wkeep, txnDoc>>

DirectoryRename(p) ==
    /\ W(p) /\ wph = "dirRename"
    /\ wph' = "txRen"
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, blobDur, record, prep,
                   removeNodes, nodeLive, wdoc, wix, wkeep, txnDoc>>

NodeRen(p) ==
    /\ W(p) /\ wph = "nodeRen"
    /\ wph' = "nodeSync"
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, blobDur, record, prep,
                   removeNodes, nodeDur, wdoc, wix, wkeep, txnDoc>>

NodeSync(p) ==
    /\ W(p) /\ wph = "nodeSync"
    /\ IF wix < NPtr
       THEN wix' = wix + 1 /\ wph' = "nodeRen"
            /\ UNCHANGED <<txnDoc, wdoc>>
       ELSE IF txnDoc < Len(removeNodes)
            THEN txnDoc' = txnDoc + 1 /\ wix' = 1 /\ wph' = "nodeRen"
                 /\ UNCHANGED wdoc
            ELSE wph' = "nodeDone" /\ txnDoc' = 0 /\ wdoc' = 0 /\ wix' = 1
    /\ UNCHANGED <<vars, ptrLive, ptrDur, blobLive, blobDur, record, prep,
                   removeNodes, nodeLive, wkeep>>

NodeDone(p) ==
    /\ W(p) /\ wph = "nodeDone"
    /\ IF pc[p] = "prepared"
       THEN LastEffect(p)
            /\ IF prep = NoPrep
               THEN wph' = "done" /\ txnDoc' = 0 /\ wdoc' = 0 /\ wix' = 1
               ELSE wph' = "collRead" /\ txnDoc' = 1 /\ wdoc' = prep[1].doc /\ wix' = 1
       ELSE wph' = "txDone" /\ txnDoc' = 1 /\ wdoc' = 0
            /\ wix' = 1 /\ UNCHANGED vars
    /\ UNCHANGED <<ptrLive, ptrDur, blobLive, blobDur, record, prep, wkeep>>

-----------------------------------------------------------------------------

\* Records, under MixedWrites, each kind of write whose residue recovery has rolled forward.
TxnTargets ==
    /\ removeNodes' = CASE wph # "planRemove" /\ wph' = "planRemove" -> Append(removeNodes, NamespaceRoot)
                       [] wph # "planRemoveAtDocument" /\ wph' = "planRemoveAtDocument" -> Append(removeNodes, DocumentEntry(1))
                       [] wph # "planRemoveBelowDocument" /\ wph' = "planRemoveBelowDocument" -> Append(removeNodes, DocumentBelow(1))
                       [] wph = "planned" /\ wph' = "done" -> NoPrep
                       [] wph = "rbDone" /\ wph' = "done" -> NoPrep
                       [] wph = "done" /\ wph' = "none" -> NoPrep
                       [] wph = "txDone" /\ wph' = "none" -> NoPrep
                       [] OTHER -> removeNodes
    /\ nodeLive' = IF crashes' # crashes THEN nodeDur
                   ELSE IF wph = "dirCreate" /\ wph' = "dirRename"
                        THEN [d \in Docs |-> [r \in Ptr |-> IF r = wix THEN TRUE ELSE nodeLive[d][r]]]
                   ELSE IF wph = "nodeRen" /\ wph' = "nodeSync"
                        THEN [d \in Docs |-> [r \in Ptr |-> IF r = wix THEN FALSE ELSE nodeLive[d][r]]]
                   ELSE nodeLive
    /\ nodeDur' = IF wph = "dirRename" /\ wph' = "txRen"
                  THEN [d \in Docs |-> [r \in Ptr |-> IF r = wix THEN nodeLive[d][r] ELSE nodeDur[d][r]]]
                  ELSE IF wph = "nodeSync" /\ wph' \in {"nodeRen", "nodeDone"}
                       THEN [d \in Docs |-> [r \in Ptr |-> IF r = wix THEN nodeLive[d][r] ELSE nodeDur[d][r]]]
                       ELSE nodeDur

    NextRunPhase(holder) ==
        IF crashes' # crashes \/ pc'[holder] = "terminated" THEN "none"
        ELSE IF pc[holder] = "recClear" /\ pc'[holder] = "closed" THEN "none"
        ELSE IF RunUnitFinished(holder) /\ pc'[holder] = "told"
              THEN CASE wph = "unitStart" -> IF directoryLive > 0 THEN "directories" ELSE "publication"
                  [] runPhase[holder] = "directories" -> IF directoryLive = 0 THEN "publication" ELSE "directories"
                [] runPhase[holder] = "publication" -> IF log' = "empty" THEN "claims" ELSE "transaction"
                 [] runPhase[holder] = "transaction" -> "collection"
                 [] runPhase[holder] = "collection" -> "claims"
                 [] OTHER -> "completion"
        ELSE IF pc[holder] \in {"held", "told"} /\ pc'[holder] \in RecStates
            THEN EffectivePhase(holder)
        ELSE runPhase[holder]

    RunState ==
        /\ runPhase' = [holder \in Procs |-> NextRunPhase(holder)]
        /\ runCount' = [holder \in Procs |->
            IF runPhase'[holder] = "none" THEN NoVal
            ELSE IF (pc[holder] \in {"held", "told"} /\ pc'[holder] \in RecStates)
                    \/ (RunUnitFinished(holder) /\ pc'[holder] = "told")
                THEN count' ELSE runCount[holder]]
        /\ unitReady' = [holder \in Procs |->
            IF pc[holder] = "locked" /\ pc'[holder] \in {"held", "told"}
                THEN requestRecovery[holder]
            ELSE IF crashes' # crashes \/ pc'[holder] = "terminated"
                 \/ (pc'[holder] = "told" /\ pc[holder] # "locked")
                 \/ (pc[holder] \in {"held", "told"} /\ pc'[holder] \in RecStates)
                THEN FALSE ELSE unitReady[holder]]

RequestIntent ==
    IF \E holder \in Procs : pc[holder] = "idle" /\ pc'[holder] = "waiting"
    THEN \E holder \in Procs, recovering \in BOOLEAN :
            /\ pc[holder] = "idle" /\ pc'[holder] = "waiting"
            /\ requestRecovery' = Set(requestRecovery, holder, recovering)
    ELSE UNCHANGED requestRecovery

Kinds ==
    rkinds' = IF ~MixedWrites THEN rkinds
              ELSE rkinds \cup (IF wph' = "rvRen" THEN {"unlogged"} ELSE {})
                          \cup (IF wph' = "txRen" THEN {"logged"} ELSE {})

DirectoryState ==
    /\ unitIntervened' = [holder \in Procs |->
          IF pc[holder] = "locked" /\ pc'[holder] \in {"held", "told"}
          THEN mygen[holder] # gen ELSE unitIntervened[holder]]
    /\ directoryTorn' = CASE wph = "dirAppend" /\ wph' = "none" -> TRUE
                         [] wph \in {"dirAppend", "dirAppendRepair"} /\ wph' = "dirAppendSync" -> FALSE
                         [] OTHER -> directoryTorn
    /\ directoryLive' = IF crashes' # crashes THEN directoryDurable
                         ELSE IF wph \in {"dirAppend", "dirAppendRepair"} /\ wph' = "dirAppendSync"
                              THEN directoryLive + 1
                         ELSE IF wph = "dirRename" /\ wph' = "txRen" THEN 0
                         ELSE IF wph = "dirClear" /\ wph' = "dirClearSync" THEN 0
                         ELSE directoryLive
    /\ directoryDurable' = IF wph = "dirAppendSync" /\ wph' = "dirCreate" THEN directoryLive
                            ELSE IF wph = "dirClearSync" /\ wph' = "dirDone" THEN directoryLive
                            ELSE directoryDurable
    /\ runDirectories' = [holder \in Procs |->
          IF crashes' # crashes \/ pc'[holder] = "terminated" THEN 0
          ELSE IF pc[holder] \in {"held", "told"} /\ pc'[holder] \in RecStates
                    /\ (runPhase[holder] = "none" \/ count # runCount[holder] \/ unitIntervened[holder]) THEN directoryLive
          ELSE IF W(holder) /\ wph = "dirSync" /\ wph' = "dirDone" THEN runDirectories[holder] - 1
          ELSE IF runPhase'[holder] \notin {"none", "directories"} THEN 0
          ELSE runDirectories[holder]]

SpNextCore ==
    \/ \E p \in Procs :
          \/ Quiet(Exit(p)) \/ Quiet(BeginAttach(p)) \/ Quiet(TakeGateS(p))
          \/ Quiet(TakeReaderS(p)) \/ Quiet(DropGate(p)) \/ Quiet(TakeWriterS(p))
          \/ SAdmit(p)
          \/ Quiet(Request(p)) \/ Quiet(Lock(p)) \/ Quiet(SetFence(p))
          \/ Quiet(Record(p))
          \/ (Quiet(BeginOpen(p)) /\ ~requestRecovery[p]) \/ Quiet(EndOpen(p)) \/ Quiet(OpenFails(p))
          \/ SEndEmpty(p) \/ SFailUnresolved(p) \/ SBeginClose(p)
          \/ Quiet(EndClose(p)) \/ Quiet(CloseFails(p))
          \/ Quiet(Release(p)) \/ Quiet(Unlock(p))
          \/ \E d \in Docs, condition \in PreconditionChoices, matches \in BOOLEAN,
                effect \in {"write", "remove"} :
                PlanTxn(p, d, condition, matches, effect)
          \/ \E d \in Docs : PlanSwapCollision(p, d)
          \/ \E marker \in {"planRemove", "planRemoveAtDocument", "planRemoveBelowDocument"} :
                PlanNodeRemoval(p, marker)
          \/ FinishNodePlan(p)
          \/ SBegin(p) \/ SReject(p) \/ SIngest(p) \/ SkipIng(p)
              \/ SPrepare(p) \/ SPrepareFails(p)
          \/ SAbort(p) \/ SEnd(p) \/ Quiet(Sweep(p)) \/ (Quiet(Quiesce(p)) /\ ~requestRecovery[p])
            \/ SRunStart(p) \/ RunUnitEnd(p) \/ Quiet(RecWhole(p)) \/ SRecSwap1(p)
              \/ DirectorySync(p) \/ DirectoryClear(p) \/ DirectoryClearSync(p)
            \/ Quiet(RecSwap2(p)) \/ SRecEnd(p) \/ SRecReset(p)
            \/ SRecSweep(p) \/ Quiet(RecCount(p)) \/ Quiet(RecClear(p))
          \/ SRecStop(p)
          \/ STerminate(p)
          \/ \E d \in Docs : Pick(p, d)
          \/ IngRen(p) \/ IngSync(p) \/ RecWrite(p) \/ RecSync(p)
          \/ PtrRen(p) \/ PtrSync(p) \/ RecClr(p) \/ RecClrSync(p)
          \/ CollRead(p) \/ CollSync(p) \/ CollRem(p) \/ CollFsync(p) \/ CollFails(p)
          \/ RvBegin(p) \/ RvRecSync(p) \/ RvRen(p) \/ RvSync(p) \/ RvClr(p)
          \/ RvClrSync(p) \/ RsRen(p) \/ RsSync(p)
          \/ RvCollRead(p) \/ RvCollSync(p) \/ RvCollRem(p) \/ RvCollFsync(p)
          \/ TxBegin(p) \/ DirectoryIntentAppend(p) \/ DirectoryIntentTorn(p)
          \/ DirectoryIntentSync(p) \/ DirectoryCreate(p) \/ DirectoryRename(p)
          \/ TxRen(p) \/ TxSync(p)
          \/ NodeRen(p) \/ NodeSync(p) \/ NodeDone(p)
            \/ RbBegin(p) \/ RbCollRead(p) \/ RbCollSync(p)
            \/ RbCollRem(p) \/ RbCollFsync(p) \/ RbCollFails(p)
    \/ SCrash
    \/ Quiet(Terminal)

WritesBounded == writes <= MaxWrites /\ ordinaryRequests <= MaxWrites

RecoveryInterleaving ==
    IF RecoverySchedule = "all" THEN TRUE
    ELSE
        /\ (\A holder \in Procs :
              (pc[holder] = "idle" /\ pc'[holder] = "waiting") =>
                  IF holder = 1
                  THEN runPhase[holder] # "completion"
                       \/ (writes = 2 /\ pc[2] = "idle")
                  ELSE runPhase[1] = "completion" /\ pc[1] = "idle")
        /\ (\A holder \in Procs :
              (pc[holder] \in RecStates /\ pc'[holder] = "told") => RunUnitFinished(holder))
        /\ (\A holder \in Procs :
              ~(pc[holder] = "open" /\ pc'[holder] = "wrote"))
        /\ ((pc[1] = "told" /\ runPhase[1] = "none" /\ requestRecovery[1])
              => pc'[1] # "releasing")
        /\ ((pc[2] = "told" /\ runPhase[2] = "none" /\ requestRecovery[2])
              => pc'[2] # "releasing")
        /\ ((pc[2] \in {"held", "told"} /\ pc'[2] \in RecStates) => writes = 1)
        /\ wph' \notin {"ptrRen", "ptrSync", "recClr", "recClrSync"}

OperationCounts ==
    /\ writes' = writes + Cardinality({holder \in Procs :
                    pc[holder] = "opening" /\ pc'[holder] = "open"})
    /\ ordinaryRequests' = ordinaryRequests + Cardinality({holder \in Procs :
                    pc[holder] = "idle" /\ pc'[holder] = "waiting" /\ ~requestRecovery'[holder]})

SpStepFrame ==
    RequestIntent /\ TxnTargets /\ RunState /\ Kinds /\ DirectoryState
    /\ RecoveryInterleaving /\ OperationCounts

SpNext == SpNextCore /\ SpStepFrame

SpSpec == SpInit /\ [][SpNext]_spvars

SpFileNext == SpNext /\ (crashes' # crashes /\ ~fence => ~fence')

SpFileSpec == SpInit /\ [][SpFileNext]_spvars

SpSharedNext ==
        /\ SpNext
        /\ \A holder \in Procs :
                    ~(runPhase[holder] = "completion" /\ pc[holder] \in {"recReset", "recCount"}
                        /\ pc'[holder] = "told")

SpSharedSpec == SpInit /\ [][SpSharedNext]_spvars

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

\* Audience: internal.
LivePointersHaveTheirNode ==
    \A d \in Docs, replica \in Ptr : ptrLive[d][replica] \in Vers => nodeLive[d][replica]

\* Audience: internal.
DurablePointersHaveTheirNode ==
    \A d \in Docs, replica \in Ptr : ptrDur[d][replica] \in Vers => nodeDur[d][replica]

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

OnlyAHolderChangesADocumentStep ==
    (docs' # docs /\ crashes' = crashes) => \E p \in Procs : W(p)

\* Audience: internal.
\* Only the holder of writer exclusion changes a document; a crash only loses what was not
\* durable. Attaching, the open of a handle, changes nothing.
OnlyAHolderChangesADocument == [][OnlyAHolderChangesADocumentStep]_spvars

-----------------------------------------------------------------------------
(* Witnesses: each is violated by a reachable state or step it names.      *)

ATransactionRejectsDigestMismatch ==
    \E p \in Procs :
        /\ pc[p] = "open" /\ pc'[p] = "wrote" /\ wph = "planned" /\ wph' = "done"
        /\ \E index \in DOMAIN prep :
             prep[index].precondition = "digest" /\ ~SwapPreconditionHolds(prep[index])
        /\ log' = log
        /\ ptrLive' = ptrLive /\ ptrDur' = ptrDur
        /\ blobLive' = blobLive /\ blobDur' = blobDur /\ record' = record

Witness_ATransactionRejectsDigestMismatch ==
    [][~ATransactionRejectsDigestMismatch]_spvars

ATransactionRejectsExpectedAbsent ==
    \E p \in Procs :
        /\ pc[p] = "open" /\ pc'[p] = "wrote" /\ wph = "planned" /\ wph' = "done"
        /\ \E index \in DOMAIN prep : prep[index].precondition = "absent"
        /\ log' = log
        /\ ptrLive' = ptrLive /\ ptrDur' = ptrDur
        /\ blobLive' = blobLive /\ blobDur' = blobDur /\ record' = record

Witness_ATransactionRejectsExpectedAbsent ==
    [][~ATransactionRejectsExpectedAbsent]_spvars

Witness_ATransactionPlansDigestPreconditions ==
    ~(Len(prep) = 2 /\ \A index \in DOMAIN prep : prep[index].precondition = "digest")

Witness_ATransactionAcceptsDigestPreconditions ==
    ~(Len(prep) = 2 /\ log = "prepared"
      /\ \A index \in DOMAIN prep : prep[index].precondition = "digest")

Witness_ATransactionPreparesTwoDocuments ==
    ~(Len(prep) = 2 /\ log = "prepared")

Witness_ACrashLeavesTwoTransactionSwapsPrepared ==
    ~(crashes = 1 /\ Len(prep) = 2)

Witness_ARecoveryAppliesTheSecondTransactionSwap ==
    ~(\E p \in Procs : pc[p] = "recSwap1" /\ wph = "txRen" /\ txnDoc = 2)

Witness_ARecoverySettlesARecordOfTheSecondDocument ==
    ~(wph = "rvRen" /\ wdoc = 2)

Witness_ARecoveryResolvesTheSecondDocument ==
    ~(wph = "rsSync" /\ wdoc = 2)

Witness_ARecoveryRollsATransactionForward ==
    ~(wph = "txRen")

Witness_ARecoveryCompletesAllUnits ==
    ~(\E holder \in Procs : pc[holder] = "recClear" /\ runPhase[holder] = "completion")

Witness_ADirectoryDebtUnitRuns == ~(wph = "dirSync")

ADirectoryClearSurvivesOnlyLive ==
    wph = "dirClearSync" /\ crashes' # crashes /\ directoryLive' > 0

Witness_ADirectoryClearSurvivesOnlyLive == [][~ADirectoryClearSurvivesOnlyLive]_spvars

ALiveRollbackCollects == wph = "rbCollRem" /\ blobLive' # blobLive

Witness_ALiveRollbackCollects == [][~ALiveRollbackCollects]_spvars

AWriteCollects == wph = "collRem" /\ blobLive' # blobLive

Witness_AWriteCollects == [][~AWriteCollects]_spvars

ARecoveryCollectsARolledBackTransaction ==
    wph = "rvCollRem" /\ log = "begun" /\ blobLive' # blobLive

Witness_ARecoveryCollectsARolledBackTransaction ==
    [][~ARecoveryCollectsARolledBackTransaction]_spvars

ARecoveryCollectsACommittedTransaction ==
    wph = "rvCollRem" /\ log = "ended" /\ blobLive' # blobLive

Witness_ARecoveryCollectsACommittedTransaction ==
    [][~ARecoveryCollectsACommittedTransaction]_spvars

ARollbackCollectionFailureRequiresRecovery ==
    /\ wph \in {"rbCollRead", "rbCollSync", "rbCollRem", "rbCollFsync"}
    /\ \E holder \in Procs : pc'[holder] = "told" /\ pc[holder] # "told"

Witness_ARollbackCollectionFailureRequiresRecovery ==
    [][~ARollbackCollectionFailureRequiresRecovery]_spvars

Witness_AWriterIsRefusedBetweenRecoveryUnits ==
    ~(\E writer, recoverer \in Procs :
        /\ writer # recoverer /\ W(writer) /\ pc[writer] = "told"
        /\ runPhase[writer] = "none"
        /\ runPhase[recoverer] # "none" /\ pc[recoverer] = "idle")

AStaleRecoveryRestarts ==
    \E holder \in Procs :
        /\ runPhase[holder] \notin {"none", "publication"}
        /\ count # runCount[holder]
        /\ pc[holder] \in {"held", "told"} /\ pc'[holder] \in RecStates
        /\ runPhase'[holder] = "publication"

Witness_AStaleRecoveryRestarts == [][~AStaleRecoveryRestarts]_spvars

Witness_AHolderTerminatesWithARecordPending ==
    ~(\E p \in Procs : pc[p] = "terminated" /\ \E d \in Docs : record[d].dur # NoVal)

Witness_RecoveryRollsForwardBothKindsOfWrite ==
    ~(rkinds = {"unlogged", "logged"})

Witness_ARestartLosesTheFenceWithARecordPending ==
    ~(terminatedIn = "crash" /\ ~fence /\ \E d \in Docs : record[d].dur # NoVal)

AWriteRemoveCollisionIsRejected ==
    /\ lockHolder \in Procs
    /\ pc'[lockHolder] = "wrote" /\ pc[lockHolder] = "open"
    /\ wph = "planned" /\ ~SwapSetHasUniqueTargets

Witness_AWriteRemoveCollisionIsRejected ==
    [][~AWriteRemoveCollisionIsRejected]_spvars

AWriteBeneathRemovedNodeIsRejected ==
    /\ lockHolder \in Procs
    /\ pc'[lockHolder] = "wrote" /\ pc[lockHolder] = "open"
    /\ wph = "planned" /\ \E path \in PlannedRemovalTargets : RemovalIsBelowWrite(path)

Witness_AWriteBeneathRemovedNodeIsRejected ==
    [][~AWriteBeneathRemovedNodeIsRejected]_spvars

ARemovalAtExistingDocumentIsRejected ==
    /\ lockHolder \in Procs
    /\ pc'[lockHolder] = "wrote" /\ pc[lockHolder] = "open"
    /\ wph = "planned" /\ DocumentEntry(1) \in PlannedRemovalTargets
    /\ DocumentAfterTransaction(1)

Witness_ARemovalAtExistingDocumentIsRejected ==
    [][~ARemovalAtExistingDocumentIsRejected]_spvars

ARemovalBeneathExistingDocumentIsRejected ==
    /\ lockHolder \in Procs
    /\ pc'[lockHolder] = "wrote" /\ pc[lockHolder] = "open"
    /\ wph = "planned" /\ DocumentBelow(1) \in PlannedRemovalTargets
    /\ DocumentAfterTransaction(1)

Witness_ARemovalBeneathExistingDocumentIsRejected ==
    [][~ARemovalBeneathExistingDocumentIsRejected]_spvars

Witness_ATransactionRemovesANode == ~(wph = "nodeDone" /\ log = "prepared")
Witness_ARecoveryRemovesANode ==
    ~(\E p \in Procs : pc[p] = "recSwap1" /\ wph = "nodeDone")

ADirectoryIntentTears == wph = "dirAppend" /\ wph' = "none" /\ directoryTorn'
Witness_ADirectoryIntentTears == [][~ADirectoryIntentTears]_spvars

ADirectoryIntentIsRepaired == wph = "dirAppendRepair" /\ wph' = "dirAppendSync"
Witness_ADirectoryIntentIsRepaired == [][~ADirectoryIntentIsRepaired]_spvars

Witness_ADirectoryIsCreatedAndRenamed ==
    ~(wph = "txRen" /\ nodeDur[1][1])

Witness_ARecoveryCreatesADirectory ==
    ~(\E p \in Procs : pc[p] = "recSwap1" /\ wph = "dirAppend")

=============================================================================
