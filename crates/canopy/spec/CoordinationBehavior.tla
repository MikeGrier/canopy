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
    /\ UNCHANGED <<schema, eligible, owner, fence, trusted, count, gen, store,
                   requests, crashes, reads>>

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
    /\ att' = [p \in Procs |-> "off"]
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
    /\ UNCHANGED <<schema, eligible, gen, store, requests, deaths, reads, lists>>

\* Every process has ended. It is the only step then, so a state in which a
\* process is stuck waiting is the only kind of deadlock.
Terminal ==
    /\ \A p \in Procs : pc[p] \in {"exited", "dead"}
    /\ UNCHANGED vars

Next ==
    \/ \E p \in Procs :
          \/ Exit(p) \/ Disqualify(p)
          \/ BeginAttach(p) \/ TakeGateS(p) \/ TakeReaderS(p) \/ DropGate(p)
          \/ KeepGate(p) \/ CheckEarly(p) \/ TakeWriterS(p) \/ Admit(p)
          \/ GiveUp(p) \/ BeginOp(p) \/ EndOp(p)
          \/ StartUpgrade(p) \/ RefuseUpgrade(p) \/ DrainDone(p)
          \/ CancelDrain(p) \/ StartUpgradeHolding(p) \/ TakeGateX(p)
          \/ RegStart(p) \/ (\E s \in Slots : RegClaim(p, s)) \/ RegLive(p)
          \/ RegWrite(p) \/ RegRelease(p) \/ RegOverflow(p)
          \/ UpdTake(p) \/ UpdDone(p) \/ UnregNone(p) \/ UnregClaim(p)
          \/ UnregSkip(p) \/ UnregClear(p) \/ UnregRelease(p) \/ Detach(p)
          \/ ListStart(p) \/ ListClaim(p) \/ ListRead(p) \/ ListProbe(p)
          \/ ListDone(p)
          \/ TakeFirstTier(p) \/ TakeSecondTier(p) \/ BeginUpgrade(p)
          \/ UpgradeMoved(p) \/ EndUpgrade(p)
          \/ Request(p) \/ Withdraw(p) \/ Refuse(p)
          \/ Lock(p) \/ SetFence(p) \/ Record(p)
          \/ BeginOpen(p) \/ EndOpen(p) \/ OpenFails(p)
          \/ FirstEffect(p) \/ LastEffect(p) \/ FailUnresolved(p)
          \/ EndEmpty(p) \/ Rollback(p)
          \/ EnterInner(p) \/ LeaveInner(p)
          \/ BeginClose(p) \/ EndClose(p) \/ CloseFails(p)
          \/ Release(p) \/ Unlock(p) \/ ClearRecord(p)
          \/ ReadBefore(p) \/ ReadBefore2(p) \/ ObserveA(p) \/ ObserveB(p)
          \/ ReadAfter(p) \/ ReadAfter1(p) \/ ReadAfter2(p) \/ Invalidate(p)
          \/ Die(p)
    \/ Crash
    \/ Terminal

Spec == Init /\ [][Next]_vars

=============================================================================
