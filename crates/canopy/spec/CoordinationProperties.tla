\* Copyright (c) Michael Grier.
\* Split from Coordination.tla at 70668aa.
----------------------- MODULE CoordinationProperties -----------------------
EXTENDS CoordinationBehavior

-----------------------------------------------------------------------------
(* Properties.                                                             *)

\* Something an acquirer can see that tells it a holder died or the state is
\* not to be trusted.
Evidence == fence \/ ~trusted \/ (owner # None /\ pc[owner] = "dead")

\* At most one live process holds writer exclusion, and the lock names it.
AtMostOneHolder ==
    Cardinality({p \in Procs : pc[p] \in HoldsLock}) <= 1

LockMatchesHolders ==
    \A p \in Procs : (pc[p] \in HoldsLock) <=> (lockHolder = p)

\* The record names the process that holds exclusion, or one that died; never
\* a process that is alive and not holding.
OwnerRecordNamesItsHolder ==
    /\ \A p \in Procs : pc[p] \in OwnsRecord => owner = p
    /\ owner # None => pc[owner] \in OwnsRecord \cup {"dead"}

\* A holder that died holding exclusion stays reported until recovery, even
\* when it died before its first effect or after its last.
AbandonmentIsNeverForgotten == diedIn # "none" => Evidence

\* Half-applied data is either its live writer's or visible as unresolved.
UnresolvedWorkIsNeverInvisible ==
    Partial => (\E p \in Procs : pc[p] \in {"writing", "nested"}) \/ Evidence

\* An abandoned record is replaced only after the fence (or the untrusted
\* state) carries what it said.
AbandonedRecordNeverReplacedUnfenced ==
    [][(owner # None /\ pc[owner] = "dead" /\ owner' # owner)
          => (fence' \/ ~trusted')]_vars

\* A holder that is not told recovery is required finds a store with nothing
\* to recover.
NormalAcquireFindsAHealthyStore ==
    [][\A p \in Procs :
          (pc[p] = "locked" /\ pc'[p] = "held")
             => (owner = None /\ ~fence /\ trusted /\ ~Partial /\ Even(count))
      ]_vars

\* An acquirer is told recovery is required only with cause, and told of an
\* abandoned record only when the record's holder really died.
ToldOnlyWithCause ==
    [][\A p \in Procs :
          (pc[p] = "locked" /\ pc'[p] = "told")
             => (owner # None \/ fence \/ ~trusted)]_vars

NoFalseAbandonmentReport ==
    [][\A p \in Procs :
          (pc[p] = "locked" /\ pc'[p] = "told" /\ owner # None)
             => pc[owner] = "dead"]_vars

\* Attaching, acquiring and a process's death change no store data: only a
\* healthy holder in the middle of a write does.
StoreDataChangesOnlyByAHealthyHolder ==
    [][store' # store =>
          \E p \in Procs : pc[p] \in {"open", "writing"} /\ lockHolder = p
                           /\ ~fence /\ trusted]_vars

\* A process's death changes none of the coordination files, only its own
\* state and the OS lock it held.
DeathLeavesTheFilesystemAsItWas ==
    [][((\E p \in Procs : pc[p] \in LiveStates /\ pc'[p] = "dead")
            /\ crashes' = crashes)
          => UNCHANGED <<schema, store, rdata, ovf, owner, fence, trusted, count,
                         gen>>]_vars

\* A host crash leaves no live process.
CrashLeavesNoLiveProcess ==
    [][crashes' # crashes => \A p \in Procs : pc'[p] \notin LiveStates]_vars

\* A request is pending exactly while its process waits.
RequestStateMatchesProcess ==
    \A p \in Procs : (req[p] = "pending") <=> (pc[p] = "waiting")

OutcomeIsJustified ==
    \A p \in Procs :
        /\ req[p] = "dead" => pc[p] = "dead"
        /\ req[p] = "refused" => ~eligible[p]

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

\* The count is odd whenever an operation is unresolved: while one is open, and
\* for as long as its data is half applied, whether or not its holder lives.
CountIsOddWhileAnOperationIsUnresolved ==
    /\ Partial => Odd(Visible)
    /\ \A p \in Procs : pc[p] \in OpenStates => Odd(Visible)

\* The count is raised only by the holder publishing it, and by nothing else
\* but a host crash.
CountIsRaisedOnlyByItsHolder ==
    [][(count' # count /\ crashes' = crashes) =>
          \E p \in Procs :
              /\ lockHolder = p
              /\ \/ pc[p] = "opening" /\ pc'[p] = "open"
                 \/ pc[p] = "closing" /\ pc'[p] = "closed"]_vars

\* Every effect is ordered after the opening raise, and before the closing one.
EffectsAreBracketedByTheCount ==
    [][store' # store => Odd(count)]_vars

\* A failed opening publication has no effect.
FailedOpeningPublicationHasNoEffect ==
    [][\A p \in Procs :
          (pc[p] = "opening" /\ pc'[p] = "closed")
             => (count' = count /\ store' = store)]_vars

\* A failed closing publication, or an operation that fails unresolved, leaves
\* the count odd and sets the fence.
FailedOperationsLeaveTheCountOddAndFence ==
    [][\A p \in Procs :
          (pc[p] \in {"closing", "writing"} /\ pc'[p] = "told")
             => (Odd(count') /\ fence')]_vars

\* A host crash is the only thing that changes the published count other than
\* its holder; and no step but an acquisition changes the generation.
GenerationAdvancesOnEveryAcquisitionAndOnlyThen ==
    [][/\ \A p \in Procs : (pc[p] = "locked" /\ pc'[p] \in {"held", "told"})
                              => gen' = gen + 1
       /\ gen' # gen => \E p \in Procs :
                           pc[p] = "locked" /\ pc'[p] \in {"held", "told"}
      ]_vars

\* A handle whose remembered generation is the current one has a cache that is
\* accurate, so it may use it without reading shared again.
GenerationMatchImpliesAnAccurateCache ==
    \A p \in Procs : (mygen[p] = gen /\ cache[p] # NoVal) => cache[p] = shared

\* A holder's cache of shared is accurate.
AHolderCacheIsAccurate ==
    \A p \in Procs : pc[p] \in OwnsRecord => cache[p] = shared

\* A cached sample is never stale.
ACachedSampleIsNeverStale ==
    \A p \in Procs : scache[p] # NoVal => scache[p] = Visible

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

\* An operation that has finished its effects, by completing or by rolling
\* back, leaves the store settled.
AFinishedOperationLeavesTheStoreSettled ==
    \A p \in Procs : pc[p] \in {"wrote", "closing", "closed"} => a = b

\* A read whose last step this is, and the value of its second sample.
Finishing(p) ==
    /\ rphase[p] \in {"after1", "after2"}
    /\ rphase'[p] = "none" /\ pc'[p] = "idle"
FinalSample(p) ==
    IF rphase[p] = "after2" THEN 2 * rpart[p] + (count % 2) ELSE Sampled(p)
Accepted(p) == Finishing(p) /\ Undisturbed(rbefore[p], FinalSample(p))

\* The read classification: a read classified as settled saw the whole of one
\* settled store, with no part of an operation in it, and nothing has changed
\* since. This is what makes a snapshot, and a verdict that a disagreement is
\* real rather than raced, sound.
SettledReadSawNoPartOfAnOperation ==
    [][\A p \in Procs :
          Accepted(p) => (rseenA[p] = a /\ rseenB[p] = b /\ a = b)]_vars

-----------------------------------------------------------------------------
(* The gate and the tiers.                                                 *)

\* An exclusive gate holder coexists with no other admission: nobody else holds
\* the gate in either mode.
AnExclusiveGateHolderCoexistsWithNoOtherAdmission ==
    /\ Cardinality({p \in Procs : HoldsGateX(p)}) <= 1
    /\ GateXHeld => ~GateSHeld

\* Tiers held exclusively are held by one handle and shared by none.
TierHoldersAreCompatible ==
    /\ Cardinality({p \in Procs : HoldsReaderX(p)}) <= 1
    /\ ReaderXHeld => ~ReaderSHeld
    /\ Cardinality({p \in Procs : HoldsWriterX(p)}) <= 1
    /\ WriterXHeld => ~WriterSHeld

\* Holders of the gate in its shared mode never block one another, and hold it
\* only until they hold their reader tier.
OrdinaryAdmissionHoldsTheGateOnlyUntilItHoldsTheReaderTier ==
    \A p \in Procs : HoldsGateS(p) => att[p] \in {"hasGate", "gateRd"}

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
            IF TakesReaderTier(p) /\ Control # "unsafe-writer-tier-first"
               THEN 2 ELSE 3
      [] att[p] = "uWr" -> 2
      [] att[p] \in {"rd", "gateWr", "rdChecked", "uRd"} -> 3
      [] pc[p] = "waiting" -> 4
      [] OTHER -> 0

NothingIsAcquiredAgainstTheOrder ==
    \A p \in Procs :
        WaitRank(p) > 0 => \A r \in HeldRanks(p) : r < WaitRank(p)

\* Writer exclusion is held only by an attached handle that may write.
ExclusionIsHeldOnlyByAttachedWriters ==
    lockHolder = None \/ (att[lockHolder] = "attached" /\ CanWrite(lockHolder))

\* No handle is admitted against the rules in force at the time, and none stays
\* attached against them as the store changes beneath it.
AdmittedHandlesAreAdmissible ==
    [][\A p \in Procs :
          (att[p] # "attached" /\ att'[p] = "attached") => Admissible(p)]_vars

AttachedHandlesStayAdmissible ==
    \A p \in Procs : att[p] = "attached" => Admissible(p)

\* An upgrade begins only from the schema its upgrader found, with no other
\* upgrade recorded in progress.
AnUpgradeBeginsFromTheSchemaItExpects ==
    [][\A p \in Procs :
          (att[p] = "uTiers" /\ att'[p] = "uBody") =>
              (current[UpLayer] = SpeaksOf[p][UpLayer] /\ upState = "ordinary")]_vars

\* The current schema moves exactly when an upgrade completes, and the oldest
\* reader schema exactly when one that is unsafe for readers does.
SchemasMoveOnlyWhenAnUpgradeCompletes ==
    [][/\ (current' # current) <=> (upState # "ordinary" /\ upState' = "ordinary")
       /\ (oldest' # oldest) <=> (upState = "unsafe" /\ upState' = "ordinary")]_vars

\* What each process's attachment state says about where it is.
AttachmentMatchesTheProcess ==
    \A p \in Procs :
        /\ pc[p] = "new" => att[p] = "off"
        /\ pc[p] \in {"exited", "dead"} => att[p] = "off" /\ lease[p] = 0
        /\ pc[p] = "detached" => att[p] = "off"
        /\ pc[p] \in LiveStates \ {"attaching", "converting", "convertingX",
                                  "detached", "detaching"}
              => att[p] = "attached"
        /\ pc[p] = "detaching" => att[p] = "unregistering"
        /\ pc[p] = "attaching" =>
              att[p] \in {"wGate", "hasGate", "gateRd", "gateWr", "rd",
                          "rdChecked", "tiers", "registering"}
        /\ pc[p] = "converting" =>
              att[p] \in {"draining", "unregistering", "uWGate", "uWGateT", "uGate",
                          "uRd", "uWr", "uTiers", "uBody"}

\* An attach or upgrade request is pending exactly while its process is
\* attaching or converting, and ends in exactly one of the four outcomes. A
\* call refused at the door is an outcome that leaves its process where it was.
AttachRequestStateMatchesProcess ==
    \A p \in Procs :
        (areq[p] = "pending")
            <=> (pc[p] \in {"attaching", "converting", "convertingX"})

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

\* An entry's liveness lock is held only by a handle that holds an attachment,
\* so an entry exists exactly while its handle is attached: not before it is
\* admitted, and not once its attachment is given up, including by a conversion.
EntriesFollowAttachment ==
    \A s \in Slots : ll[s] # None => att[ll[s]] \in HoldsAttachment

\* A handle that is attached has an entry it holds, or the registry says that
\* some handle attached without one.
AttachedHandlesAreRegisteredOrAnnounced ==
    RegistryOn =>
        \A p \in Procs :
            att[p] \in {"attached", "draining"} =>
                \/ rst[p] \in {"in", "upd"}
                \/ rst[p] = "unreg" /\ ovf

ARegisteredHandleOwnsItsEntry ==
    \A p \in Procs :
        rst[p] \in {"in", "upd"} =>
            (ll[rcur[p]] = p /\ etok[rcur[p]] = p)

AnUnregisteredHandleIsAnnounced ==
    (\E p \in Procs : rst[p] = "unreg") => ovf

\* With no claim lock held, an entry's token names the handle that holds its
\* liveness lock: no one can see a new owner's lock beside an old owner's token.
AFreeClaimShowsTheTokenOfTheLivenessHolder ==
    \A s \in Slots : (cl[s] = None /\ ll[s] # None) => etok[s] = ll[s]

\* A death releases the locks of the process that died, and a clean exit has
\* already released its own.
RegistryLocksAreHeldOnlyByLiveProcesses ==
    \A s \in Slots :
        /\ cl[s] # None => pc[cl[s]] \notin {"dead", "exited"}
        /\ ll[s] # None => pc[ll[s]] \notin {"dead", "exited"}

\* Every change to an entry is made by the handle that holds its claim lock, and
\* what it writes is its own token, or a clearing. A crash discards the entries
\* with the rest of the coordination state.
EntryChangesAreMadeUnderTheClaimLock ==
    [][\A s \in Slots :
          ((etok'[s] # etok[s] \/ edr'[s] # edr[s]) /\ crashes' = crashes) =>
              (/\ cl[s] # None
               /\ (etok'[s] = etok[s] \/ etok'[s] \in {None, cl[s]}))]_vars

ACrashDiscardsTheRegistry ==
    [][crashes' # crashes =>
          (\A s \in Slots : etok'[s] = None /\ ~edr'[s]) /\ ~ovf']_vars

\* Every inspection of an entry is made under its claim lock.
EntryInspectionsAreMadeUnderTheClaimLock ==
    [][\A p \in Procs :
          ((lph[p] = "held" /\ lph'[p] = "read")
           \/ (lph[p] = "read" /\ lph'[p] = "next")) =>
              cl[lcur[p]] = p]_vars

\* A listing reports an entry live only if the liveness lock was held, as of its
\* probe, by the handle the token names, and that handle holds an attachment.
LiveReportIsJustified ==
    [][\A p \in Procs :
          (lph[p] = "read" /\ lph'[p] = "next" /\ lrep'[p][3] = "live") =>
              (ll[lrep'[p][1]] = lrep'[p][2]
               /\ att[lrep'[p][2]] \in HoldsAttachment)]_vars

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

\* No operation is in flight beneath a conversion: the drain waited for every
\* lease before the upgrader released its tiers.
NoLeaseIsHeldOnceAConversionBegins ==
    \A p \in Procs :
        att[p] \in ConversionWaits \cup {"uBody"} =>
            (lease[p] = 0 /\ rphase[p] = "none")

\* An operation is in flight only while its handle holds the tiers it was
\* admitted under, so no upgrade completes beneath it.
AnOperationInFlightHoldsTheTiers ==
    \A p \in Procs :
        (lease[p] > 0 \/ rphase[p] # "none") =>
            (HoldsReaderS(p) /\ (CanWrite(p) => HoldsWriterS(p)))

\* An upgrade never waits for an operation that its own call is blocking.
NoDrainWaitsOnItsOwnCaller ==
    \A p \in Procs : att[p] = "draining" => rphase[p] = "none"

\* An upgrade is never started from inside an operation of its handle.
AnUpgradeNeverStartsFromInsideAnOperation ==
    [][\A p \in Procs :
          (pc[p] \notin {"converting", "convertingX"}
           /\ pc'[p] \in {"converting", "convertingX"}) => ~InsideOperation(p)]_vars

OperationStarts(p) ==
    \/ lease'[p] > lease[p]
    \/ rphase[p] = "none" /\ rphase'[p] # "none"
    \/ pc[p] = "idle" /\ pc'[p] = "waiting"

\* Neither a clone nor the handle starts an operation while its handle drains,
\* converts or is detached.
NoOperationStartsWhileTheHandleConverts ==
    [][\A p \in Procs : OperationStarts(p) => att[p] = "attached"]_vars

\* Cancelling the drain ends it and leaves the handle attached and usable.
ACancelledDrainLeavesTheHandleAttached ==
    [][\A p \in Procs :
          (att[p] = "draining" /\ areq'[p] = "withdrawn") =>
              (att'[p] = "attached" /\ pc'[p] = "idle")]_vars

\* Once the drain has finished the attachment is the upgrader's to give up, so a
\* conversion that is cancelled or fails leaves the handle detached.
ACancelledOrFailedConversionLeavesTheHandleDetached ==
    [][\A p \in Procs :
          (att[p] \in ConversionWaits /\ pc[p] = "converting"
           /\ areq'[p] \in {"withdrawn", "refused"}) =>
              (att'[p] = "off" /\ pc'[p] = "detached")]_vars

=============================================================================
