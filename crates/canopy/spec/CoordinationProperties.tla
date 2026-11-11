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
        pc[p] \in {"wrote", "closing", "closed"} =>
            (a = b /\ log \in {"empty", "ended"} /\ stage = 0)

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

-----------------------------------------------------------------------------
(* The gate and the tiers.                                                 *)

\* Audience: internal.
\* An exclusive gate holder coexists with no other admission: nobody else holds
\* the gate in either mode.
AnExclusiveGateHolderCoexistsWithNoOtherAdmission ==
    /\ Cardinality({p \in Procs : HoldsGateX(p)}) <= 1
    /\ GateXHeld => ~GateSHeld

\* Audience: internal.
\* Tiers held exclusively are held by one handle and shared by none.
TierHoldersAreCompatible ==
    /\ Cardinality({p \in Procs : HoldsReaderX(p)}) <= 1
    /\ ReaderXHeld => ~ReaderSHeld
    /\ Cardinality({p \in Procs : HoldsWriterX(p)}) <= 1
    /\ WriterXHeld => ~WriterSHeld

\* Audience: internal.
\* Holders of the gate in its shared mode never block one another, and hold it
\* only until they hold their reader tier.
OrdinaryAdmissionHoldsTheGateOnlyUntilItHoldsTheReaderTier ==
    \A p \in Procs : HoldsGateS(p) => att[p] \in {"hasGate", "gateRd"}

\* Audience: internal.
NoHandleWaitsForTheGateWhileHoldingATier ==
    \A p \in Procs :
        att[p] \in {"wGate", "uWGate", "uWGateT"} =>
            ~(HoldsReaderS(p) \/ HoldsWriterS(p)
              \/ HoldsReaderX(p) \/ HoldsWriterX(p))

\* The resources are ranked gate, reader tier, writer tier, writer exclusion,
\* and a process waits only for one ranked above every one it holds.
HeldRanks(p) ==
    {r \in 1..4 :
        \/ r = 1 /\ (HoldsGateS(p) \/ HoldsGateX(p))
        \/ r = 2 /\ (HoldsReaderS(p) \/ HoldsReaderX(p))
        \/ r = 3 /\ (HoldsWriterS(p) \/ HoldsWriterX(p))
        \/ r = 4 /\ lockHolder = p}

WaitRank(p) ==
    CASE att[p] \in {"wGate", "uWGate", "uWGateT"} -> 1
      [] att[p] = "hasGate" -> 2
      [] att[p] = "uGate" ->
            IF DecideReaderTier(p) /\ Control # "unsafe-writer-tier-first"
               THEN 2 ELSE 3
      [] att[p] = "uWr" -> 2
      [] att[p] \in {"rd", "gateWr", "rdChecked", "uRd"} -> 3
      [] pc[p] = "waiting" -> 4
      [] OTHER -> 0

\* Audience: internal.
NothingIsAcquiredAgainstTheOrder ==
    \A p \in Procs :
        WaitRank(p) > 0 => \A r \in HeldRanks(p) : r < WaitRank(p)

\* Audience: client. Only an attached handle that may write holds writer exclusion. Shows through:
\* a handle that may not write, writing.
\* Writer exclusion is held only by an attached handle that may write, or by an
\* upgrader in its body.
ExclusionIsHeldOnlyByAttachedWriters ==
    lockHolder = None
    \/ (att[lockHolder] \in {"attached", "uBody"} /\ CanWrite(lockHolder))

\* Audience: client. A handle is admitted only if the rules in force admit it. Shows through: the
\* outcome of attach.
\* No handle is admitted against the rules in force at the time, and none stays
\* attached against them as the store changes beneath it.
AdmittedHandlesAreAdmissible ==
    [][\A p \in Procs :
          (att[p] # "attached" /\ att'[p] = "attached") => Admissible(p)]_vars

\* Audience: client. A handle that stays attached keeps meeting the rules in force as the store
\* changes beneath it. Shows through: an attached handle that reads or writes data it cannot.
AttachedHandlesStayAdmissible ==
    \A p \in Procs : att[p] = "attached" => Admissible(p)

\* Audience: client. An upgrade begins only from the schema its caller found, with no other
\* upgrade in progress; otherwise it is refused. Shows through: the outcome of an upgrade.
\* An upgrade begins only from the schema its upgrader found, with no other
\* upgrade recorded in progress.
AnUpgradeBeginsFromTheSchemaItExpects ==
    [][\A p \in Procs :
          (att[p] = "uTiers" /\ att'[p] = "uBody") =>
              (current[UpL(p)] = SpeaksOf[p][UpL(p)] /\ upState = "ordinary")]_vars

\* Audience: client. The store's schema changes exactly when an upgrade completes, and its oldest
\* admitted reader schema exactly when one that is unsafe for readers does. Shows through: the
\* schema a store reports and the handles it admits.
\* The current schema moves exactly when an upgrade completes, and the oldest
\* reader schema exactly when one that is unsafe for readers does.
SchemasMoveOnlyWhenAnUpgradeCompletes ==
    [][UpSteps = 0 =>
          /\ (current' # current) <=> (upState # "ordinary" /\ upState' = "ordinary")
          /\ (oldest' # oldest) <=> (upState = "unsafe" /\ upState' = "ordinary")]_vars

\* What each process's attachment state says about where it is. An upgrader in
\* its body is in one of the states of a holder.
BodyStates == {"locked", "held", "opening", "open", "upBody", "wrote", "closing",
               "closed", "told", "releasing"}

\* Audience: internal.
AttachmentMatchesTheProcess ==
    \A p \in Procs :
        /\ pc[p] = "new" => att[p] = "off"
        /\ pc[p] \in {"exited", "dead"} => att[p] = "off" /\ lease[p] = 0
        /\ pc[p] = "detached" => att[p] = "off"
        /\ pc[p] \in LiveStates \ {"attaching", "converting", "convertingX",
                                  "detached", "detaching"}
              => att[p] \in {"attached", "uBody"}
        /\ att[p] = "uBody" => Upgrader(p)
        /\ (UpSteps > 0 /\ att[p] = "uBody") => pc[p] \in BodyStates
        /\ pc[p] = "detaching" => att[p] = "unregistering"
        /\ pc[p] = "attaching" =>
              att[p] \in {"wGate", "hasGate", "gateRd", "gateWr", "rd",
                          "rdChecked", "tiers", "registering"}
        /\ pc[p] = "converting" =>
              att[p] \in {"draining", "unregistering", "uWGate", "uWGateT", "uGate",
                          "uRd", "uWr", "uTiers", "uBody"}

\* Audience: internal.
\* An attach or upgrade request is pending exactly while its process is
\* attaching or converting, and ends in exactly one of the four outcomes. A
\* call refused at the door is an outcome that leaves its process where it was.
AttachRequestStateMatchesProcess ==
    \A p \in Procs :
        (areq[p] = "pending")
            <=> (pc[p] \in {"attaching", "converting", "convertingX"}
                 \/ att[p] = "uBody")

\* Audience: client. Every attach or upgrade request ends in exactly one outcome and keeps it.
\* Shows through: the outcome of attach and of upgrade.
AttachRequestsEndInExactlyOneOutcome ==
    [][\A p \in Procs :
          /\ areq[p] = "none" => areq'[p] \in {"none", "pending", "refused"}
          /\ areq[p] \in Outcomes =>
                areq'[p] \in {areq[p], "pending", "refused"}
          /\ (areq[p] # "pending" /\ areq'[p] \notin {areq[p], "pending"}) =>
                (areq'[p] = "refused" /\ pc'[p] = pc[p])
          /\ (areq[p] = "pending" /\ areq'[p] # "pending") =>
                \/ areq'[p] = "served" /\ pc'[p] \in {"idle", "exited", "held"}
                \/ areq'[p] = "dead" /\ pc'[p] = "dead"
                \/ areq'[p] = "withdrawn"
                      /\ pc'[p] \in {"idle", "exited", "detached"}
                \/ areq'[p] = "refused"
                      /\ pc'[p] \in {"exited", "held", "detached"}
      ]_vars

-----------------------------------------------------------------------------
(* The registry.                                                           *)

\* Audience: client. The registry lists a handle exactly while it is attached. Shows through: the
\* registry listing.
\* An entry's liveness lock is held only by a handle that holds an attachment,
\* so an entry exists exactly while its handle is attached: not before it is
\* admitted, and not once its attachment is given up, including by a conversion.
EntriesFollowAttachment ==
    \A s \in Slots : ll[s] # None => att[ll[s]] \in HoldsAttachment

\* Audience: client. An attached handle is in the registry, or the registry says that some handle
\* is missing from it. Shows through: the registry listing.
\* A handle that is attached has an entry it holds, or the registry says that
\* some handle attached without one.
AttachedHandlesAreRegisteredOrAnnounced ==
    RegistryOn =>
        \A p \in Procs :
            att[p] \in {"attached", "draining"} =>
                \/ rst[p] \in {"in", "upd"}
                \/ rst[p] = "unreg" /\ ovf

\* Audience: internal.
ARegisteredHandleOwnsItsEntry ==
    \A p \in Procs :
        rst[p] \in {"in", "upd"} =>
            (ll[rcur[p]] = p /\ etok[rcur[p]] = p)

\* Audience: internal.
AnUnregisteredHandleIsAnnounced ==
    (\E p \in Procs : rst[p] = "unreg") => ovf

\* Audience: internal.
\* With no claim lock held, an entry's token names the handle that holds its
\* liveness lock: no one can see a new owner's lock beside an old owner's token.
AFreeClaimShowsTheTokenOfTheLivenessHolder ==
    \A s \in Slots : (cl[s] = None /\ ll[s] # None) => etok[s] = ll[s]

\* Audience: internal.
\* A death releases the locks of the process that died, and a clean exit has
\* already released its own.
RegistryLocksAreHeldOnlyByLiveProcesses ==
    \A s \in Slots :
        /\ cl[s] # None => pc[cl[s]] \notin {"dead", "exited"}
        /\ ll[s] # None => pc[ll[s]] \notin {"dead", "exited"}

\* Audience: internal.
\* Every change to an entry is made by the handle that holds its claim lock, and
\* what it writes is its own token, or a clearing. A crash discards the entries
\* with the rest of the coordination state.
EntryChangesAreMadeUnderTheClaimLock ==
    [][\A s \in Slots :
          ((etok'[s] # etok[s] \/ edr'[s] # edr[s]) /\ crashes' = crashes) =>
              (/\ cl[s] # None
               /\ (etok'[s] = etok[s] \/ etok'[s] \in {None, cl[s]}))]_vars

\* Audience: internal.
ACrashDiscardsTheRegistry ==
    [][crashes' # crashes =>
          (\A s \in Slots : etok'[s] = None /\ ~edr'[s]) /\ ~ovf']_vars

\* Audience: internal.
\* Every inspection of an entry is made under its claim lock.
EntryInspectionsAreMadeUnderTheClaimLock ==
    [][\A p \in Procs :
          ((lph[p] = "held" /\ lph'[p] = "read")
           \/ (lph[p] = "read" /\ lph'[p] = "next")) =>
              cl[lcur[p]] = p]_vars

\* Audience: client. A listing reports a handle live only if it holds an attachment.
\* Shows through: the registry listing.
\* A listing reports an entry live only if the liveness lock was held, as of its
\* probe, by the handle the token names, and that handle holds an attachment.
LiveReportIsJustified ==
    [][\A p \in Procs :
          (lph[p] = "read" /\ lph'[p] = "next" /\ lrep'[p][3] = "live") =>
              (ll[lrep'[p][1]] = lrep'[p][2]
               /\ att[lrep'[p][2]] \in HoldsAttachment)]_vars

\* Audience: client. A listing says it may be incomplete when it could not read an entry or some
\* handle is attached without one. Shows through: the registry listing.
\* A listing says it may be incomplete when it met an entry it could not claim,
\* and when some handle is attached without an entry.
AListingSaysWhenItMayBeIncomplete ==
    [][\A p \in Procs :
          (lph[p] = "next" /\ lph'[p] = "none" /\ pc'[p] # "dead"
           /\ crashes' = crashes) =>
              /\ lunk[p] => lres'[p] = "incomplete"
              /\ (\E q \in Procs : att[q] \in {"attached", "draining"}
                                  /\ rst[q] = "unreg") =>
                    lres'[p] = "incomplete"]_vars

-----------------------------------------------------------------------------
(* Clones, leases and the drain.                                           *)

ConversionWaits == {"uWGate", "uWGateT", "uGate", "uRd", "uWr", "uTiers"}

\* Audience: client. No operation is in flight beneath an upgrade. Shows through: an upgrade that
\* runs under operations or waits for them.
\* No operation is in flight beneath a conversion: the drain waited for every
\* lease before the upgrader released its tiers.
NoLeaseIsHeldOnceAConversionBegins ==
    \A p \in Procs :
        att[p] \in ConversionWaits \cup {"uBody"} =>
            (lease[p] = 0 /\ rphase[p] = "none")

\* Audience: internal.
\* An operation is in flight only while its handle holds the tiers it was
\* admitted under, so no upgrade completes beneath it.
AnOperationInFlightHoldsTheTiers ==
    \A p \in Procs :
        (lease[p] > 0 \/ rphase[p] # "none") =>
            (HoldsReaderS(p) /\ (CanWrite(p) => HoldsWriterS(p)))

\* Audience: client. An upgrade never waits for an operation that its own call is blocking.
\* Shows through: a call that never returns.
\* An upgrade never waits for an operation that its own call is blocking.
NoDrainWaitsOnItsOwnCaller ==
    \A p \in Procs : att[p] = "draining" => rphase[p] = "none"

\* Audience: client. An upgrade called from inside an operation of its handle is refused.
\* Shows through: the outcome of an upgrade.
\* An upgrade is never started from inside an operation of its handle.
AnUpgradeNeverStartsFromInsideAnOperation ==
    [][\A p \in Procs :
          (pc[p] \notin {"converting", "convertingX"}
           /\ pc'[p] \in {"converting", "convertingX"}) => ~InsideOperation(p)]_vars

OperationStarts(p) ==
    \/ lease'[p] > lease[p]
    \/ rphase[p] = "none" /\ rphase'[p] # "none"
    \/ pc[p] = "idle" /\ pc'[p] = "waiting"

\* Audience: client. No operation starts, on the handle or on a clone, while the handle upgrades
\* or is detached. Shows through: the outcome of an operation.
\* Neither a clone nor the handle starts an operation while its handle drains,
\* converts or is detached.
NoOperationStartsWhileTheHandleConverts ==
    [][\A p \in Procs : OperationStarts(p) => att[p] = "attached"]_vars

\* Audience: client. A cancelled drain leaves the handle attached and usable. Shows through: the
\* state of the handle after a cancelled upgrade.
\* Cancelling the drain ends it and leaves the handle attached and usable.
ACancelledDrainLeavesTheHandleAttached ==
    [][\A p \in Procs :
          (att[p] = "draining" /\ areq'[p] = "withdrawn") =>
              (att'[p] = "attached" /\ pc'[p] = "idle")]_vars

\* Audience: client. A conversion cancelled or failed after the drain leaves the handle detached.
\* Shows through: the state of the handle after a cancelled or failed upgrade.
\* Once the drain has finished the attachment is the upgrader's to give up, so a
\* conversion that is cancelled or fails leaves the handle detached.
ACancelledOrFailedConversionLeavesTheHandleDetached ==
    [][\A p \in Procs :
          (att[p] \in ConversionWaits /\ pc[p] = "converting"
           /\ areq'[p] \in {"withdrawn", "refused"}) =>
              (att'[p] = "off" /\ pc'[p] = "detached")]_vars

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

-----------------------------------------------------------------------------
(* The upgrade body.                                                       *)

\* Audience: internal.
\* An upgrader finds writer exclusion free: it holds the writer tier
\* exclusively, so no handle that may write is attached to hold or want it.
AnUpgraderFindsExclusionFree ==
    [][\A p \in Procs :
          (UpSteps > 0 /\ att[p] = "uTiers" /\ att'[p] = "uBody") =>
              lockHolder = None]_vars

\* Audience: internal.
\* A migration step is never durable without the intent that says to finish it,
\* so an upgrader that dies after its first step leaves recovery something to
\* resume.
AMigrationStepIsNeverUnannounced == ustep > 0 => upState # "ordinary"

\* Audience: internal.
\* Progress only grows while the intent stands.
UpgradeProgressNeverGoesBack ==
    [][(upState # "ordinary" /\ upState' # "ordinary") => ustep' >= ustep]_vars

\* Audience: client. A layer's schema version advances only after every migration step is done.
\* Shows through: the schema a store reports.
\* The layer's schema advances only after every migration step.
TheVersionAdvancesOnlyAfterEveryStep ==
    [][(UpSteps > 0 /\ current' # current) =>
          (upState # "ordinary" /\ ustep = UpSteps)]_vars

\* Audience: client. An upgrade unsafe for readers stops admitting the older readers, and one safe
\* for them does not. Shows through: the outcome of attach.
\* The oldest reader schema moves with the schema, and only for an upgrade that
\* is unsafe for readers.
TheOldestReaderSchemaMovesWithTheSchema ==
    [][UpSteps > 0 =>
          ((oldest' # oldest) <=> (current' # current /\ upState = "unsafe"))]_vars

\* Audience: client. An interrupted upgrade is resumed to completion and never undone.
\* Shows through: the schema a store reports after recovery.
\* The intent clears only once the schema has advanced, so an interrupted upgrade
\* is resumed to completion and never undone.
TheIntentClearsOnlyAfterTheSchemaAdvances ==
    [][(UpSteps > 0 /\ upState # "ordinary" /\ upState' = "ordinary") =>
          Advanced]_vars

\* Audience: internal.
\* The upgrade's effects are made under exclusion, by its upgrader in its body or
\* by a recoverer, and inside the count's bracket.
UpgradeEffectsAreMadeUnderExclusionAndTheCount ==
    [][(UpSteps > 0 /\ (upState' # upState \/ ustep' # ustep \/ current' # current))
          => /\ Odd(Visible)
             /\ \E p \in Procs :
                   lockHolder = p /\ pc[p] \in {"open", "upBody", "recUp"}]_vars

\* Audience: client. Only a handle that speaks the target schema resumes an interrupted upgrade.
\* Shows through: the outcome of recovery.
\* An interrupted upgrade is resumed only by a handle that speaks the target
\* schema.
AResumptionIsMadeOnlyByAHandleThatSpeaksTheTarget ==
    [][\A p \in Procs :
          (pc[p] = "told" /\ pc'[p] = "recUp") =>
              SpeaksOf[p][ulayer] = utgt]_vars

\* Audience: client. While an upgrade is in progress no handle that speaks only the source schema
\* writes. Shows through: writes from a handle that is too old.
\* While an upgrade's intent stands, whoever holds exclusion is its upgrader or
\* speaks the target schema, so no handle that speaks only the source schema
\* writes.
NoHandleThatSpeaksOnlyTheSourceHoldsExclusionDuringAnUpgrade ==
    IntentStands =>
        \A p \in Procs :
            lockHolder = p =>
                (att[p] = "uBody" \/ SpeaksOf[p][ulayer] = utgt)

\* Audience: client. An upgrade whose body completes is served; one that does not is refused and
\* leaves its handle detached. Shows through: the outcome of an upgrade.
\* A body that completes serves its upgrade and one that does not refuses it and
\* leaves its handle detached.
ABodyEndsServedOrRefused ==
    [][\A p \in Procs :
          (UpSteps > 0 /\ att[p] = "uBody" /\ att'[p] = "off"
           /\ pc'[p] # "dead") =>
              IF Completed(p) THEN pc'[p] = "exited" /\ areq'[p] = "served"
                              ELSE pc'[p] = "detached" /\ areq'[p] = "refused"]_vars

-----------------------------------------------------------------------------
(* Cumulative reader compatibility.                                        *)

\* Audience: client. An upgrade is safe for readers exactly when the oldest reader schema the
\* store admits can read what it leaves. Shows through: the outcome of an upgrade and the readers
\* admitted after it.
\* An upgrade is judged against the oldest reader schema the store still admits:
\* it is safe for readers exactly when that schema can read what it leaves, and
\* every schema above it can then read it too.
AnUpgradeIsJudgedAgainstTheOldestSchema ==
    [][(ulayer = 0 /\ ulayer' # 0) =>
          ((upState' = "safe") <=> (ufloor' <= oldest[ulayer']))]_vars

\* Audience: client. An upgrade of one layer changes nothing in another. Shows through: the
\* schemas a store reports.
\* An upgrade of one layer changes nothing in another: neither its schemas nor the
\* floor of its data.
AnUpgradeOfOneLayerLeavesTheOthersAlone ==
    [][\A l \in Layers :
          l # ulayer => /\ current'[l] = current[l]
                        /\ oldest'[l] = oldest[l]
                        /\ dfloor'[l] = dfloor[l]]_vars

\* Audience: client. Every reader schema the store admits can read its data. Shows through: attach
\* and reads.
\* The oldest reader schema the store admits never falls below what the data in a
\* layer requires, so every schema it admits can read the data.
OldestCoversTheDataFloor == \A l \in Layers : dfloor[l] <= oldest[l]

\* Audience: client. While one layer upgrades, the admission rules of every other layer are
\* unchanged. Shows through: the outcome of attach.
\* Whatever upgrade is in progress, the rules of every other layer are its ordinary
\* ones: a reader needs a schema from the oldest to the current, and a writer the
\* current.
TheOtherLayersKeepTheirOrdinaryRules ==
    \A p \in Procs, l \in Layers :
        l # ulayer =>
            LET s == SpeaksOf[p][l] IN
            /\ ReaderOK(l, s) <=> (oldest[l] <= s /\ s <= current[l])
            /\ WriterOK(l, s) <=> s = current[l]

\* Audience: client. An attached handle can read the data in every layer, whatever completes after
\* it attached. Shows through: reads.
\* An attached handle speaks a schema that can read the data in every layer,
\* whatever has completed since it attached.
AnAttachedHandleCanReadTheData ==
    \A p \in Procs :
        att[p] = "attached" => \A l \in Layers : dfloor[l] <= SpeaksOf[p][l]

=============================================================================
