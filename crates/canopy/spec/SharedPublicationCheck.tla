\* Copyright (c) Michael Grier.
----------------------- MODULE SharedPublicationCheck ----------------------
EXTENDS SharedPublication

ASSUME MaxReads = 0 /\ MaxUpgrades = 0 /\ ~Callbacks /\ ~Notifications

ReferencedVersions ==
    ({0, gen, prep.ver}
     \cup {ptrLive[document][replica] : document \in Docs, replica \in Ptr}
     \cup {ptrDur[document][replica] : document \in Docs, replica \in Ptr}
     \cup (UNION {blobLive[document][replica] : document \in Docs, replica \in Blob})
     \cup (UNION {blobDur[document][replica] : document \in Docs, replica \in Blob})
     \cup {record[document].live : document \in Docs}
     \cup {record[document].dur : document \in Docs}
     \cup wkeep) \cap Vers

VersionRank(version) ==
    IF version \in Vers
    THEN Cardinality({prior \in ReferencedVersions : prior < version})
    ELSE version

RankedVersions(versions) == {VersionRank(version) : version \in versions}

RankedDocuments ==
    <<[document \in Docs |-> [replica \in Ptr |-> VersionRank(ptrLive[document][replica])]],
      [document \in Docs |-> [replica \in Ptr |-> VersionRank(ptrDur[document][replica])]],
      [document \in Docs |-> [replica \in Blob |-> RankedVersions(blobLive[document][replica])]],
      [document \in Docs |-> [replica \in Blob |-> RankedVersions(blobDur[document][replica])]],
      [document \in Docs |-> [live |-> VersionRank(record[document].live),
                              dur |-> VersionRank(record[document].dur)]],
      [doc |-> prep.doc, ver |-> VersionRank(prep.ver)]>>

SavedCountClasses ==
    [holder \in Procs |-> IF runCount[holder] = NoVal THEN "none"
                          ELSE IF runCount[holder] = count THEN "current" ELSE "stale"]

RelativeTransaction ==
    <<a - b, log, stage # 0, IF stage = 0 THEN 0 ELSE stage - a,
      ptgt # 0, IF ptgt = 0 THEN 0 ELSE ptgt - a>>

RememberedGenerations ==
    [holder \in Procs |-> mygen[holder] = gen]

RelevantRequestIntent ==
    [holder \in Procs |-> pc[holder] \notin {"new", "idle", "exited", "terminated"}
                          /\ requestRecovery[holder]]

RelevantUnitReadiness ==
    [holder \in Procs |-> pc[holder] \in {"held", "told"} /\ unitReady[holder]]

RelevantIntervention ==
    [holder \in Procs |-> pc[holder] \in {"held", "told"} /\ unitReady[holder]
                          /\ requestRecovery[holder] /\ unitIntervened[holder]]

QuotientView ==
    <<adm, pc, req, lockHolder, owner, fence, trusted, count % 2,
      VersionRank(gen), RelativeTransaction, reader, terminatedIn, counts, RankedDocuments,
      wph, wdoc, wix, RankedVersions(wkeep), txnDoc, runPhase, SavedCountClasses,
    RelevantUnitReadiness, RelevantRequestIntent, directoryLive, directoryDurable, runDirectories,
    RelevantIntervention, RememberedGenerations,
    writes, ordinaryRequests, rkinds>>

BoundedTypes == INSTANCE SharedPublication
    WITH gen <- 0, mygen <- [holder \in Procs |-> 0]

\* Audience: internal.
QuotientTypeOK ==
    /\ BoundedTypes!SpTypeOK
    /\ gen \in Nat
    /\ mygen \in [Procs -> Nat \cup {NoVal}]
    /\ \A holder \in Procs : pc[holder] \in {"opening", "open", "begun", "ingested", "prepared", "writing", "applied", "ended", "wrote", "closing"} => ~requestRecovery[holder]

\* Audience: internal.
DocumentWorkUsesCurrentGeneration ==
    wph # "none" => (lockHolder # None /\ mygen[lockHolder] = gen)

\* Audience: internal.
SavedRecoveryCountNeverLeads ==
    \A holder \in Procs : runPhase[holder] # "none" => runCount[holder] <= count

\* Audience: internal.
StoredVersionsNeverLeadTheGeneration ==
    \A version \in ReferencedVersions : version <= gen

\* Audience: internal.
OnlyOrdinaryRequestsConsumeTheBudget == requests = ordinaryRequests

IsRecoveryRequest ==
    \E holder \in Procs : requestRecovery'[holder]
        /\ pc[holder] = "idle" /\ pc'[holder] = "waiting"

RecoveryRequest(holder) ==
    /\ CanWrite(holder)
    /\ pc[holder] = "idle" /\ rphase[holder] = "none"
    /\ pc' = Set(pc, holder, "waiting")
    /\ req' = Set(req, holder, "pending")
    /\ requestRecovery' = Set(requestRecovery, holder, TRUE)
    /\ UNCHANGED <<adm, coord, store, handle, reader, terminatedIn, counts,
                   docs, work, writes, ordinaryRequests>>
    /\ TxnTargets /\ RunState /\ Kinds /\ DirectoryState /\ RecoveryInterleaving

QuotientNext ==
    \/ SpNext /\ ~IsRecoveryRequest
    \/ \E holder \in Procs : RecoveryRequest(holder)

QuotientSpec == SpInit /\ [][QuotientNext]_spvars

=============================================================================