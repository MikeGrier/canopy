\* Copyright (c) Michael Grier.
\* Split from CoordinationProperties.tla at 18b499a.
------------------ MODULE CoordinationAttachmentProperties ------------------
EXTENDS CoordinationProperties

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

=============================================================================
