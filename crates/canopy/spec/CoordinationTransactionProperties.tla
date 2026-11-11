\* Copyright (c) Michael Grier.
\* Split from CoordinationProperties.tla at 18b499a.
----------------- MODULE CoordinationTransactionProperties ------------------
EXTENDS CoordinationAttachmentProperties

-----------------------------------------------------------------------------
(* Transactions and the commit log.                                        *)

\* Audience: internal.
\* The prepare record carries its swap set exactly while it is the log's latest.
PreparedRecordCarriesItsSwapSet == (log = "prepared") <=> (ptgt # 0)

\* Audience: internal.
\* A prepare record means the bodies it moves to were staged first, and they stay
\* staged until the transaction has ended.
ABodyIsStagedBeforeItIsPrepared == log = "prepared" => stage = ptgt

\* Audience: internal.
\* Staged bodies are always announced by a log record, so that recovery can find
\* and sweep them.
StagingIsAlwaysAnnouncedByTheLog ==
    stage # 0 => log \in {"begun", "prepared", "ended"}

\* Audience: internal.
\* Where a transaction's holder is says what the log and the staging hold.
TransactionStateMatchesTheLog ==
    Txn =>
        \A p \in Procs :
            /\ pc[p] = "begun" => (log = "begun" /\ stage = 0)
            /\ pc[p] = "ingested" => (log = "begun" /\ stage # 0)
            /\ pc[p] \in {"prepared", "writing", "nested", "applied"} =>
                  (log = "prepared" /\ ptgt # 0 /\ stage = ptgt)
            /\ pc[p] = "ended" => log = "ended"

\* Audience: internal.
\* The namespace moves only once the prepare record is in the log.
TheNamespaceMovesOnlyAfterPrepare ==
    [][Txn => ((a' # a \/ b' # b) => log = "prepared")]_vars

\* Audience: internal.
\* The end record is written only after every swap has landed.
AnEndRecordFollowsEverySwap ==
    [][(log = "prepared" /\ log' = "ended") => (a = ptgt /\ b = ptgt)]_vars

\* Audience: internal.
\* Resetting the log never discards a prepared transaction, which the log alone
\* lets recovery finish.
AResetNeverDiscardsAPreparedTransaction ==
    [][~(log = "prepared" /\ log' = "empty")]_vars

\* Audience: internal.
\* The log changes only under writer exclusion.
TheLogChangesOnlyUnderExclusion == [][log' # log => lockHolder # None]_vars

\* Audience: internal.
\* A transaction begins only from a settled store, so it never buries unresolved
\* work.
ANewTransactionStartsFromASettledStore ==
    [][\A p \in Procs :
          (pc[p] = "open" /\ pc'[p] = "begun") =>
              (log \in {"empty", "ended"} /\ stage = 0 /\ a = b
               /\ ~fence /\ trusted)]_vars

\* Audience: client. A transaction that fails before it is prepared leaves the store as it was.
\* Shows through: the outcome of a failed transaction and the next read.
\* A failure before the prepare record is written leaves nothing behind: the
\* namespace is as it was, the staging swept and the log reset.
AFailureBeforePrepareLeavesNothingBehind ==
    [][\A p \in Procs :
          (pc[p] \in {"begun", "ingested"} /\ pc'[p] = "wrote") =>
              (a' = a /\ b' = b /\ stage' = 0 /\ log' = "empty")]_vars

-----------------------------------------------------------------------------
(* Recovery and discovery.                                                 *)

\* Audience: client. A store that needs recovery says so until a recovery has finished.
\* Shows through: the outcome of the next write and of discovery.
\* The fence is cleared only by a recovery that has finished -- the store settled
\* and the count published even and trusted -- or lost with the rest of the
\* coordination state in a host crash, which leaves that state untrusted.
TheFenceIsClearedOnlyByAFinishedRecovery ==
    [][(fence /\ ~fence' /\ crashes' = crashes) =>
          \E p \in Procs :
              pc[p] = "recClear" /\ Settled /\ Even(count) /\ trusted]_vars

\* Audience: internal.
\* Trust is restored only by a recovery that has finished, when it publishes the
\* count.
TrustIsRestoredOnlyByAFinishedRecovery ==
    [][(~trusted /\ trusted') =>
          \E p \in Procs : pc[p] = "recCount" /\ Settled]_vars

\* Audience: internal.
\* Recovery publishes the count only once the store is settled, so no sample of an
\* even count covers work recovery has not finished.
RecoveryPublishesTheCountOnlyOnceTheStoreIsSettled ==
    [][\A p \in Procs :
          (pc[p] = "recCount" /\ pc'[p] # "recCount") => Settled]_vars

\* Audience: client. After a recovery finishes, the next writer is not told to recover.
\* Shows through: the outcome of the next write and of discovery.
\* A recovery's last step leaves the fence clear, so the next acquirer finds a
\* store it is not told to recover.
RecoveryClearsTheFenceAtItsEnd ==
    [][\A p \in Procs :
          (pc[p] = "recClear" /\ pc'[p] = "closed") => ~fence']_vars

\* Audience: client. Discovery reports recovery required only when the store needs it; the
\* transaction of a live writer is not abandoned. Shows through: the report discovery returns.
\* Discovery reports recovery required only with cause: the fence set, state that
\* cannot be vouched for, or the record of a holder that died. A live holder's
\* transaction is not abandoned.
DiscoveryReportsRequiredOnlyWithCause ==
    [][\A p \in Procs :
          Discover(p) => (disc'[p] = "required" => Evidence)]_vars

\* Audience: client. With no writer active, discovery always reports abandoned work.
\* Shows through: the report discovery returns.
\* With no one holding exclusion, abandoned work is always reported.
DiscoveryReportsAbandonedWorkWhenNobodyHoldsExclusion ==
    [][\A p \in Procs :
          Discover(p) =>
              ((lockHolder = None /\ Evidence) => disc'[p] = "required")]_vars

\* Audience: client. Asking whether recovery is required changes nothing. Shows through: the
\* report discovery returns and the store that follows it.
\* Discovery changes nothing but the report it hands back, so asking whether
\* recovery is required is never what makes it unnecessary.
DiscoveryMutatesNothing ==
    [][\A p \in Procs :
          Discover(p) =>
              UNCHANGED <<adm, pc, req, eligible, coord, store, handle, rphase,
                          rbefore, rseenA, rseenB, rpart, scache, diedIn,
                          requests, deaths, crashes, reads, upgrades>>]_vars

=============================================================================
