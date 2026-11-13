\* Copyright (c) Michael Grier.
\* Split from Coordination.tla at 70668aa.
----------------------- MODULE CoordinationProperties -----------------------
EXTENDS CoordinationBehavior

-----------------------------------------------------------------------------
(* Properties.                                                             *)

\* Something an acquirer can see that tells it a holder died or the state is
\* not to be trusted.
Evidence == fence \/ ~trusted \/ (owner # None /\ pc[owner] = "dead")

\* Audience: client. Only one process writes a store at a time. Shows through: lost or interleaved
\* updates.
\* At most one live process holds writer exclusion, and the lock names it.
AtMostOneHolder ==
    Cardinality({p \in Procs : pc[p] \in HoldsLock}) <= 1

\* Audience: internal.
LockMatchesHolders ==
    \A p \in Procs : (pc[p] \in HoldsLock) <=> (lockHolder = p)

\* Audience: internal.
\* The record names the process that holds exclusion, or one that died; never
\* a process that is alive and not holding.
OwnerRecordNamesItsHolder ==
    /\ \A p \in Procs : pc[p] \in OwnsRecord => owner = p
    /\ owner # None => pc[owner] \in OwnsRecord \cup {"dead"}

\* Audience: client. If a writer dies partway, the next writer is told recovery is required, even
\* when it died before its first change or after its last. Shows through: the outcome of the next
\* write and of discovery.
\* A holder that died holding exclusion stays reported until recovery, even
\* when it died before its first effect or after its last.
AbandonmentIsNeverForgotten == diedIn # "none" => Evidence

\* Audience: client. Half-done work is never hidden: it belongs to a live writer, or the next
\* writer and discovery are told recovery is required. Shows through: the outcome of the next
\* write and of discovery.
\* Half-applied data, an unresolved log record, staged bodies and a standing
\* upgrade intent are each their live holder's or visible as unresolved.
UnresolvedWorkIsNeverInvisible ==
    UnresolvedWork =>
        (\E p \in Procs : pc[p] \in TxnStates \cup RecStates \cup {"upBody"})
        \/ Evidence

\* Audience: internal.
\* An abandoned record is replaced only after the fence (or the untrusted
\* state) carries what it said.
AbandonedRecordNeverReplacedUnfenced ==
    [][(owner # None /\ pc[owner] = "dead" /\ owner' # owner)
          => (fence' \/ ~trusted')]_vars

\* Audience: client. A writer that is not told recovery is required finds a store with nothing to
\* recover. Shows through: a write that goes on over half-applied data.
\* A holder that is not told recovery is required finds a store with nothing
\* to recover.
NormalAcquireFindsAHealthyStore ==
    [][\A p \in Procs :
          (pc[p] = "locked" /\ pc'[p] = "held")
             => (owner = None /\ ~fence /\ trusted /\ ~Partial /\ Even(count)
                 /\ ~IntentStands /\ log \in {"empty", "ended"} /\ stage = 0)
      ]_vars

\* Audience: client. A writer is told recovery is required only when there is cause.
\* Shows through: a write refused with recovery required when nothing needs recovering.
\* An acquirer is told recovery is required only with cause, and told of an
\* abandoned record only when the record's holder really died.
ToldOnlyWithCause ==
    [][\A p \in Procs :
          (pc[p] = "locked" /\ pc'[p] = "told")
             => (owner # None \/ fence \/ ~trusted)]_vars

\* Audience: client. A writer is told that a holder died only if it did. Shows through: the
\* recovery-required outcome that names an abandoned holder.
NoFalseAbandonmentReport ==
    [][\A p \in Procs :
          (pc[p] = "locked" /\ pc'[p] = "told" /\ owner # None)
             => pc[owner] = "dead"]_vars

\* Audience: client. Attaching, acquiring and a process dying never change the data; only a writer
\* that owes no recovery, or a recovery, does. Shows through: data that changed without a write.
\* Attaching, acquiring and a process's death change no store data: only a
\* healthy holder in the middle of a write, or resetting the log, does, and a
\* recoverer, which has a capability the others do not.
StoreDataChangesOnlyByAHealthyHolder ==
    [][store' # store =>
          \E p \in Procs :
              /\ lockHolder = p
              /\ \/ (pc[p] \in {"open", "held", "closed"} \cup TxnStates
                    /\ ~fence /\ trusted)
                 \/ pc[p] \in RecStates]_vars

\* Audience: internal.
\* A process's death changes none of the coordination files, only its own
\* state and the OS lock it held.
DeathLeavesTheFilesystemAsItWas ==
    [][((\E p \in Procs : pc[p] \in LiveStates /\ pc'[p] = "dead")
            /\ crashes' = crashes)
          => UNCHANGED <<recorded, store, rdata, ovf, owner, fence, trusted, count,
                         gen>>]_vars

\* Audience: internal.
\* A host crash leaves no live process.
CrashLeavesNoLiveProcess ==
    [][crashes' # crashes => \A p \in Procs : pc'[p] \notin LiveStates]_vars

\* Audience: internal.
\* A request is pending exactly while its process waits.
RequestStateMatchesProcess ==
    \A p \in Procs : (req[p] = "pending") <=> (pc[p] = "waiting")

\* Audience: client. A request is reported dead only if its process died, and refused only if the
\* rules did not admit it. Shows through: the outcome of a request.
OutcomeIsJustified ==
    \A p \in Procs :
        /\ req[p] = "dead" => pc[p] = "dead"
        /\ req[p] = "refused" => ~eligible[p]

\* Audience: client. Every request ends in exactly one outcome -- served, refused, withdrawn or
\* dead -- and keeps it. Shows through: the outcome of a request.
\* A request leaves "pending" in exactly one of the four outcomes, which then
\* does not change until the process makes another request, and each outcome
\* comes with the process state that explains it.
EveryRequestEndsInExactlyOneOutcome ==
    [][\A p \in Procs :
          /\ req[p] = "none" => req'[p] \in {"none", "pending"}
          /\ req[p] \in Outcomes => req'[p] \in {req[p], "pending"}
          /\ (pc[p] = "waiting" /\ pc'[p] # "waiting") =>
                \/ pc'[p] = "locked" /\ req'[p] = "served" /\ eligible[p]
                \/ pc'[p] = "dead" /\ req'[p] = "dead"
                \/ pc'[p] = "idle" /\ req'[p] = "withdrawn"
                \/ pc'[p] = "idle" /\ req'[p] = "refused" /\ ~eligible[p]
      ]_vars

\* Audience: internal.
\* The count is odd whenever an operation is unresolved: while one is open, and
\* for as long as its data is half applied, its log unresolved, its bodies staged
\* or an upgrade intent standing, whether or not its holder lives.
CountIsOddWhileAnOperationIsUnresolved ==
    /\ UnresolvedWork => Odd(Visible)
    /\ \A p \in Procs : pc[p] \in OpenStates => Odd(Visible)

\* Audience: internal.
\* The count is raised only by the holder publishing it, and by nothing else
\* but a host crash.
CountIsRaisedOnlyByItsHolder ==
    [][(count' # count /\ crashes' = crashes) =>
          \E p \in Procs :
              /\ lockHolder = p
              /\ \/ pc[p] = "opening" /\ pc'[p] = "open"
                 \/ pc[p] = "closing" /\ pc'[p] = "closed"
                 \/ pc[p] = "recCount"]_vars

\* Audience: internal.
\* Every effect is ordered after the opening raise, and before the closing one.
EffectsAreBracketedByTheCount ==
    [][(a' # a \/ b' # b) => Odd(Visible)]_vars

\* Audience: client. A write that fails before it begins changes nothing. Shows through: a failed
\* write that left a trace.
\* A failed opening publication has no effect.
FailedOpeningPublicationHasNoEffect ==
    [][\A p \in Procs :
          (pc[p] = "opening" /\ pc'[p] = "closed")
             => (count' = count /\ store' = store)]_vars

\* Audience: internal.
\* A failed closing publication, or an operation that fails unresolved, leaves
\* the count odd and sets the fence.
FailedOperationsLeaveTheCountOddAndFence ==
    [][\A p \in Procs :
          (pc[p] \in {"closing", "writing", "ingested", "prepared", "applied", "ended",
                      "upBody"}
           /\ pc'[p] = "told")
             => (Odd(count') /\ fence')]_vars

\* Audience: internal.
\* A host crash is the only thing that changes the published count other than
\* its holder; and no step but an acquisition changes the generation.
GenerationAdvancesOnEveryAcquisitionAndOnlyThen ==
    [][/\ \A p \in Procs : (pc[p] = "locked" /\ pc'[p] \in {"held", "told"})
                              => gen' = gen + 1
       /\ gen' # gen => \E p \in Procs :
                           pc[p] = "locked" /\ pc'[p] \in {"held", "told"}
      ]_vars

\* Audience: internal.
\* A handle whose remembered generation is the current one has a cache that is
\* accurate, so it may use it without reading shared again.
GenerationMatchImpliesAnAccurateCache ==
    \A p \in Procs : (mygen[p] = gen /\ cache[p] # NoVal) => cache[p] = shared

\* Audience: internal.
\* A holder's cache of shared is accurate.
AHolderCacheIsAccurate ==
    \A p \in Procs : pc[p] \in OwnsRecord => cache[p] = shared

\* Audience: internal.
\* A cached sample is never stale.
ACachedSampleIsNeverStale ==
    \A p \in Procs : scache[p] # NoVal => scache[p] = Visible

\* Audience: internal.
\* The reader's bookkeeping is consistent with where it is.
ReadersAreConsistent ==
    \A p \in Procs :
        /\ rphase[p] = "none" =>
              (rbefore[p] = NoVal /\ rseenA[p] = NoVal /\ rseenB[p] = NoVal
               /\ rpart[p] = NoVal)
        /\ rphase[p] \in {"obsA", "obsB", "after1", "after2"} =>
              rbefore[p] # NoVal
        /\ rphase[p] \in {"obsB", "after1", "after2"} => rseenA[p] # NoVal
        /\ rphase[p] \in {"after1", "after2"} => rseenB[p] # NoVal
        /\ rphase[p] # "none" => pc[p] = "idle"

\* Audience: client. A write that has finished, completed or rolled back, leaves no half-applied
\* data, unresolved log record or staged bodies. Shows through: the next read and discovery.
\* An operation that has finished its effects, by completing or by rolling
\* back, leaves the store settled: its log resolved and its staging swept.
AFinishedOperationLeavesTheStoreSettled ==
    \A p \in Procs :
        pc[p] \in {"wrote", "observed", "closing", "closed"} =>
            (a = b /\ log \in {"empty", "ended"} /\ stage = 0)

\* Audience: internal.
\* A callback the store makes under exclusion changes nothing but where its holder is: it
\* does not call into the store, so neither the store, the coordination state nor any
\* other process sees it.
ACallbackChangesOnlyWhereItsHolderIs ==
    [][\A p \in Procs :
          (pc[p] = "wrote" /\ pc'[p] = "observed") =>
              /\ \A q \in Procs \ {p} : pc'[q] = pc[q]
              /\ UNCHANGED <<req, eligible, coord, store, handle, reader, diedIn, counts,
                             att, areq, lease, schema, reg>>]_vars

\* A read whose last step this is, and the value of its second sample.
Finishing(p) ==
    /\ rphase[p] \in {"after1", "after2"}
    /\ rphase'[p] = "none" /\ pc'[p] = "idle"
FinalSample(p) ==
    IF rphase[p] = "after2" THEN 2 * rpart[p] + (count % 2) ELSE Sampled(p)
Accepted(p) == Finishing(p) /\ Undisturbed(rbefore[p], FinalSample(p))

\* Audience: client. A read reported as settled saw the whole of one settled store, and nothing
\* changed beneath it. Shows through: the classification of a read.
\* The read classification: a read classified as settled saw the whole of one
\* settled store, with no part of an operation in it, and nothing has changed
\* since. This is what makes a snapshot, and a verdict that a disagreement is
\* real rather than raced, sound.
SettledReadSawNoPartOfAnOperation ==
    [][\A p \in Procs :
          Accepted(p) => (rseenA[p] = a /\ rseenB[p] = b /\ a = b)]_vars

\* Audience: internal.
\* What a waiter samples is what it waits for: Ready, which a notification is woken by, holds
\* exactly when one of the steps that wait is enabled, so the two cannot drift apart.
ReadyMatchesTheWaits == \A p \in Procs : Waiting(p) => (Ready(p) <=> ENABLED WaitSteps(p))

=============================================================================
