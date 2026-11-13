\* Copyright (c) Michael Grier.
\* Split from Coordination.tla at 70668aa.
------------------------ MODULE CoordinationBehavior ------------------------
EXTENDS CoordinationOperations

-----------------------------------------------------------------------------
(* Death and crash.                                                        *)

\* EXTERNAL: one process dies at any step. The OS releases its lock; nothing
\* it left in the filesystem changes, and the ownership record and the count
\* stay as published.
Die(p) ==
    /\ deaths < MaxDeaths
    /\ pc[p] \in LiveStates
    /\ pc' = Set(pc, p, "dead")
    /\ lockHolder' = IF lockHolder = p THEN None ELSE lockHolder
    /\ req' = IF pc[p] = "waiting" THEN Set(req, p, "dead") ELSE req
    /\ diedIn' = IF owner = p THEN pc[p] ELSE diedIn
    /\ deaths' = deaths + 1
    /\ att' = Set(att, p, "off")
    /\ rtier' = Set(rtier, p, FALSE)
    /\ areq' = IF areq[p] = "pending" THEN Set(areq, p, "dead") ELSE areq
    /\ lease' = Set(lease, p, 0)
    /\ Forget(p)
    /\ cl' = [s \in Slots |-> IF cl[s] = p THEN None ELSE cl[s]]
    /\ ll' = [s \in Slots |-> IF ll[s] = p THEN None ELSE ll[s]]
    /\ rst' = Set(rst, p, "out")
    /\ rscan' = Set(rscan, p, {})
    /\ rcur' = Set(rcur, p, 0)
    /\ lph' = Set(lph, p, "none")
    /\ lcur' = Set(lcur, p, 1)
    /\ ltok' = Set(ltok, p, None)
    /\ lrep' = Set(lrep, p, NoRep)
    /\ lunk' = Set(lunk, p, FALSE)
    /\ lfree' = Set(lfree, p, FALSE)
    /\ lres' = Set(lres, p, "none")
    /\ UNCHANGED <<rdata, ovf, lists>>
    /\ UNCHANGED <<recorded, eligible, owner, fence, trusted, count, gen, store,
                   requests, crashes, reads, discoveries, upgrades, woken>>

\* EXTERNAL: power loss. Every process dies and every lock is released. The
\* coordination state cannot be vouched for afterwards, and the record, the
\* fence and the count may or may not have survived: the count may roll back
\* over a writer that was mid-operation.
Crash ==
    /\ crashes < MaxCrashes
    /\ pc' = [p \in Procs |-> IF pc[p] \in LiveStates THEN "dead" ELSE pc[p]]
    /\ req' = [p \in Procs |-> IF pc[p] = "waiting" THEN "dead" ELSE req[p]]
    /\ lockHolder' = None
    /\ owner' \in {owner, None}
    /\ fence' \in {fence, FALSE}
    /\ trusted' = FALSE
    /\ count' \in {count, IF Odd(count) THEN count - 1 ELSE count}
    /\ diedIn' = IF owner # None THEN "crash" ELSE diedIn
    /\ crashes' = crashes + 1
    /\ mygen' = [p \in Procs |-> NoVal]
    /\ cache' = [p \in Procs |-> NoVal]
    /\ rphase' = [p \in Procs |-> "none"]
    /\ rbefore' = [p \in Procs |-> NoVal]
    /\ rseenA' = [p \in Procs |-> NoVal]
    /\ rseenB' = [p \in Procs |-> NoVal]
    /\ rpart' = [p \in Procs |-> NoVal]
    /\ scache' = [p \in Procs |-> NoVal]
    /\ disc' = [p \in Procs |-> "none"]
    /\ att' = [p \in Procs |-> "off"]
    /\ rtier' = [p \in Procs |-> FALSE]
    /\ areq' = [p \in Procs |-> IF areq[p] = "pending" THEN "dead" ELSE areq[p]]
    /\ lease' = [p \in Procs |-> 0]
    /\ cl' = [s \in Slots |-> None]
    /\ ll' = [s \in Slots |-> None]
    /\ etok' = [s \in Slots |-> None]
    /\ edr' = [s \in Slots |-> FALSE]
    /\ ovf' = FALSE
    /\ rst' = [p \in Procs |-> "out"]
    /\ rscan' = [p \in Procs |-> {}]
    /\ rcur' = [p \in Procs |-> 0]
    /\ lph' = [p \in Procs |-> "none"]
    /\ lcur' = [p \in Procs |-> 1]
    /\ ltok' = [p \in Procs |-> None]
    /\ lrep' = [p \in Procs |-> NoRep]
    /\ lunk' = [p \in Procs |-> FALSE]
    /\ lfree' = [p \in Procs |-> FALSE]
    /\ lres' = [p \in Procs |-> "none"]
    /\ UNCHANGED <<recorded, eligible, gen, store, requests, deaths, reads, lists,
                   discoveries, upgrades, woken>>

-----------------------------------------------------------------------------
(* Notifications (DESIGN-NOTES "A change to the coordination state is      *)
(* delivered as a notification"). Under Notifications a waiter acts on     *)
(* what it waits for only while it samples; one that finds it absent       *)
(* registers and sleeps, and a notification, a step of its own, wakes it   *)
(* to sample again.                                                        *)

\* Where a process waits for the coordination state to change: for writer
\* exclusion, the gate, a tier, or the end of its clones' operations.
Waiting(p) ==
    \/ pc[p] = "waiting"
    \/ att[p] \in {"wGate", "hasGate", "rd", "gateWr", "rdChecked", "uWGate",
                  "uWGateT", "uGate", "uRd", "uWr", "draining"}

\* The steps that wait, each taken only once what it waits for is there.
WaitSteps(p) ==
    \/ Lock(p) \/ TakeGateS(p) \/ TakeReaderS(p) \/ TakeWriterS(p) \/ TakeGateX(p)
    \/ TakeFirstTier(p) \/ TakeSecondTier(p) \/ DrainDone(p)

\* What a waiter waits for is there: one of WaitSteps(p) is enabled, which
\* ReadyMatchesTheWaits checks.
Ready(p) ==
    \/ pc[p] = "waiting" /\ eligible[p] /\ lockHolder = None
    \/ att[p] = "wGate" /\ ~GateXHeld
    \/ att[p] = "hasGate" /\ ~ReaderXHeld
    \/ /\ \/ att[p] = "rd" /\ Control # "check-before-writer-tier"
          \/ att[p] \in {"gateWr", "rdChecked"}
       /\ ~WriterXHeld
    \/ att[p] \in {"uWGate", "uWGateT"} /\ ~GateSHeld /\ ~GateXHeld
    \/ /\ att[p] = "uGate"
       /\ IF DecideReaderTier(p) /\ Control # "unsafe-writer-tier-first"
             THEN ~ReaderSHeld /\ ~ReaderXHeld
             ELSE ~WriterSHeld /\ ~WriterXHeld
    \/ att[p] = "uRd" /\ ~WriterSHeld /\ ~WriterXHeld
    \/ att[p] = "uWr" /\ ~ReaderSHeld /\ ~ReaderXHeld
    \/ /\ att[p] = "draining" /\ rst[p] \in {"out", "in", "unreg"}
       /\ (Leases(p) = 0 \/ Control = "drain-ignores-leases")

\* A waiter acts on what it waits for only while it samples.
Sampling(p) == ~Notifications \/ woken[p] = "sample"

\* Everything but the notices.
Unnoticed == <<pc, att, areq, lease, schema, reg, req, eligible, coord, store, handle,
               reader, diedIn, requests, deaths, crashes, reads, discoveries, upgrades>>

\* A waiter that samples and finds what it waits for absent registers and sleeps.
Register(p) ==
    /\ Notifications /\ woken[p] = "sample" /\ Waiting(p) /\ ~Ready(p)
    /\ woken' = Set(woken, p, "asleep")
    /\ UNCHANGED Unnoticed

\* A change to what a sleeping waiter waits for wakes it, to sample again; under
\* Control = "notification-spurious" it may be woken with nothing changed.
Notify(p) ==
    /\ Notifications /\ woken[p] = "asleep" /\ Waiting(p)
    /\ (Ready(p) \/ Control = "notification-spurious")
    /\ woken' = Set(woken, p, "sample")
    /\ UNCHANGED Unnoticed

\* Under Control = "notification-lost" a sleeping waiter's notification is lost.
LoseNotification(p) ==
    /\ Notifications /\ Control = "notification-lost" /\ woken[p] = "asleep"
    /\ woken' = Set(woken, p, "lost")
    /\ UNCHANGED Unnoticed

\* Every process has ended. It is the only step then, so a state in which a
\* process is stuck waiting is the only kind of deadlock.
Terminal ==
    /\ \A p \in Procs : pc[p] \in {"exited", "dead"}
    /\ UNCHANGED vars

Next ==
    \/ \E p \in Procs :
          \/ Exit(p) \/ Disqualify(p)
          \/ BeginAttach(p) \/ DropGate(p) \/ KeepGate(p) \/ CheckEarly(p) \/ Admit(p)
          \/ GiveUp(p) \/ BeginOp(p) \/ EndOp(p)
          \/ StartUpgrade(p) \/ RefuseUpgrade(p)
          \/ CancelDrain(p) \/ StartUpgradeHolding(p)
          \/ Sampling(p) /\ WaitSteps(p)
          \/ Register(p) \/ Notify(p) \/ LoseNotification(p)
          \/ RegStart(p) \/ (\E s \in Slots : RegClaim(p, s)) \/ RegLive(p)
          \/ RegWrite(p) \/ RegRelease(p) \/ RegOverflow(p)
          \/ UpdTake(p) \/ UpdDone(p) \/ UnregNone(p) \/ UnregClaim(p)
          \/ UnregSkip(p) \/ UnregClear(p) \/ UnregRelease(p) \/ Detach(p)
          \/ ListStart(p) \/ ListClaim(p) \/ ListRead(p) \/ ListProbe(p)
          \/ ListDone(p)
          \/ BeginUpgrade(p) \/ UpgradeMoved(p) \/ EndUpgrade(p)
          \/ Request(p) \/ Withdraw(p) \/ Refuse(p)
          \/ SetFence(p) \/ Record(p)
          \/ BeginOpen(p) \/ EndOpen(p) \/ OpenFails(p)
          \/ Begin(p) \/ Ingest(p) \/ Prepare(p) \/ PrepareFails(p) \/ Abort(p)
          \/ End(p) \/ Sweep(p) \/ Quiesce(p)
          \/ RecStart(p) \/ RecWhole(p) \/ RecSwap1(p) \/ RecSwap2(p) \/ RecEnd(p) \/ RecReset(p)
          \/ RecSweep(p) \/ RecCount(p) \/ RecClear(p) \/ RecStop(p) \/ Discover(p)
          \/ RecordIntent(p) \/ UpStep(p) \/ UpAdvance(p) \/ UpClear(p) \/ UpFails(p)
          \/ FirstEffect(p) \/ LastEffect(p) \/ FailUnresolved(p)
          \/ EndEmpty(p) \/ Rollback(p)
          \/ Observe(p) \/ BeginClose(p) \/ EndClose(p) \/ CloseFails(p)
          \/ Release(p) \/ Unlock(p) \/ ClearRecord(p)
          \/ ReadBefore(p) \/ ReadBefore2(p) \/ ObserveA(p) \/ ObserveB(p)
          \/ ReadAfter(p) \/ ReadAfter1(p) \/ ReadAfter2(p) \/ Invalidate(p)
          \/ Die(p)
    \/ Crash
    \/ Terminal

Spec == Init /\ [][Next]_vars

=============================================================================
