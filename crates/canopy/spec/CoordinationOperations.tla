\* Copyright (c) Michael Grier.
\* Split from Coordination.tla at 70668aa.
----------------------- MODULE CoordinationOperations -----------------------
EXTENDS CoordinationAttachment

-----------------------------------------------------------------------------
(* Writer exclusion (DESIGN-NOTES "Writer exclusion", "Ownership").        *)

\* The request is served when the lock is granted. Whenever it is free this is
\* enabled for every eligible waiter.
Lock(p) ==
    /\ pc[p] = "waiting" /\ eligible[p]
    /\ lockHolder = None
    /\ lockHolder' = p
    /\ pc' = Set(pc, p, "locked")
    /\ req' = Set(req, p, "served")
    /\ UNCHANGED <<adm, eligible, owner, fence, trusted, count, gen, store, handle,
                   reader, diedIn, counts>>

\* Under the lock the ownership record, the fence and the trust flag cannot
\* change. A record found here was left by a holder that died, because a clean
\* release clears the record before it drops the lock.

\* The fence is set before the abandoned record is replaced, so the evidence
\* outlives the acquirer's own clean release.
SetFence(p) ==
    /\ pc[p] = "locked" /\ owner # None /\ ~fence
    /\ Control # "replace-without-fence"
    /\ fence' = TRUE
    /\ UNCHANGED <<adm, pc, req, eligible, lockHolder, owner, trusted, count, gen,
                   store, handle, reader, diedIn, counts>>

\* The acquirer records itself as holder, advances the generation, and is told
\* that recovery is required when it found a record, the fence set, or state it
\* cannot vouch for. It keeps its cache of shared only if the generation it
\* found is the one it last advanced to. It recovers nothing, and a told holder
\* writes nothing.
Record(p) ==
    /\ pc[p] = "locked"
    /\ (owner = None \/ fence \/ Control = "replace-without-fence")
    /\ owner' = p
    /\ gen' = Nx(gen)
    /\ mygen' = Set(mygen, p, Nx(gen))
    /\ cache' = Set(cache, p,
                    IF cache[p] # NoVal
                          /\ (mygen[p] = gen \/ Control = "cache-always-kept")
                       THEN cache[p] ELSE shared)
    /\ pc' = Set(pc, p,
                 IF (owner # None \/ fence
                       \/ (~trusted /\ Control # "ignore-restart"))
                    /\ ~(att[p] = "uBody"
                         /\ Control = "upgrade-ignores-recovery-required")
                    THEN "told" ELSE "held")
    /\ UNCHANGED <<adm, req, eligible, lockHolder, fence, trusted, count, store,
                   reader, diedIn, counts>>

\* The opening raise is prepared, then published.
BeginOpen(p) ==
    /\ pc[p] = "held"
    /\ pc' = Set(pc, p, "opening")
    /\ UNCHANGED <<adm, req, eligible, coord, store, handle, reader, diedIn, counts>>

\* Publishing makes the new count visible at once, and any cached sample of the
\* old one is invalidated before the publication takes effect.
Cleared == IF Control = "late-invalidation" THEN scache
           ELSE [q \in Procs |-> NoVal]

EndOpen(p) ==
    /\ pc[p] = "opening"
    /\ count' = Nx(count)
    /\ pc' = Set(pc, p, "open")
    /\ scache' = Cleared
    /\ UNCHANGED <<adm, req, eligible, lockHolder, owner, fence, trusted, gen, store,
                   handle, rphase, rbefore, rseenA, rseenB, rpart, disc, diedIn,
                   counts>>

\* A failed opening publication makes no effect and leaves the count as it was.
OpenFails(p) ==
    /\ pc[p] = "opening"
    /\ pc' = Set(pc, p, "closed")
    /\ UNCHANGED <<adm, req, eligible, coord, store, handle, reader, diedIn, counts>>

\* Where a transaction's first swap is made from: after the prepare record, or
\* under a control before it.
ApplyFrom ==
    IF Control = "apply-before-prepare" THEN {"prepared", "ingested"}
    ELSE {"prepared"}

\* Where a write's first effect is made from: a transaction's, an unlogged
\* write's, or under MixedWrites either.
EffectFrom ==
    IF Txn THEN ApplyFrom
    ELSE IF MixedWrites THEN ApplyFrom \cup {"open"}
    ELSE {"open"}

\* Whether the write between its effects is a transaction. Only a transaction
\* leaves its prepare record as the log's latest while it makes its swaps.
TxnOp == Txn \/ (MixedWrites /\ log = "prepared")

\* Whether a transaction may be begun.
Logs == Txn \/ MixedWrites

\* An abstract write's first and last effects change the two cells in turn, and
\* the first changes shared, and the holder's cache with it. A transaction's are
\* its swaps, which move the cells to the version its prepare record names.
FirstEffect(p) ==
    /\ att[p] # "uBody"
    /\ pc[p] \in EffectFrom
    /\ pc' = Set(pc, p, "writing")
    /\ IF pc[p] # "open"
          THEN /\ a' = IF ptgt # 0 THEN ptgt ELSE stage
               /\ UNCHANGED <<shared, cache>>
          ELSE /\ a' = Nx(a)
               /\ shared' = Nx(shared)
               /\ cache' = Set(cache, p, Nx(shared))
    /\ UNCHANGED <<adm, req, eligible, coord, b, log, stage, ptgt, mygen, reader,
                   diedIn, counts>>

LastEffect(p) ==
    /\ pc[p] = "writing"
    /\ pc' = Set(pc, p, IF TxnOp THEN "applied" ELSE "wrote")
    /\ b' = IF TxnOp THEN (IF ptgt # 0 THEN ptgt ELSE stage) ELSE Nx(b)
    /\ UNCHANGED <<adm, req, eligible, coord, a, shared, log, stage, ptgt, handle,
                   reader, diedIn, counts>>

\* An operation that finds nothing to change ends without an effect, and one
\* that fails after its first effect but can be rolled back undoes it; either
\* then publishes the closing raise, so the count is even again.
EndEmpty(p) ==
    /\ pc[p] = "open"
    /\ pc' = Set(pc, p, "wrote")
    /\ UNCHANGED <<adm, req, eligible, coord, store, handle, reader, diedIn, counts>>

Rollback(p) ==
    /\ ~TxnOp /\ pc[p] = "writing"
    /\ pc' = Set(pc, p, "wrote")
    /\ a' = b
    /\ shared' = Pv(shared)
    /\ cache' = Set(cache, p, Pv(shared))
    /\ UNCHANGED <<adm, req, eligible, coord, b, log, stage, ptgt, mygen, reader,
                   diedIn, counts>>

\* An operation that fails after its first effect and cannot be rolled back sets
\* the fence and leaves the count odd. A transaction cannot be rolled back once
\* its prepare record may be in the log, and fails unresolved from then on.
FailStates ==
    IF ~Logs THEN {"writing"}
    ELSE IF Control = "failure-after-prepare-is-rolled-back" THEN {"ended"}
    ELSE {"prepared", "writing", "applied", "ended"}

FailUnresolved(p) ==
    /\ pc[p] \in FailStates
    /\ pc' = Set(pc, p, "told")
    /\ fence' = TRUE
    /\ UNCHANGED <<adm, req, eligible, lockHolder, owner, trusted, count, gen, store,
                   handle, reader, diedIn, counts>>

\* The prepare record's append fails, and the record may or may not have been
\* written, so the failure is ambiguous and unresolved.
PrepareFails(p) ==
    /\ Logs /\ pc[p] = "ingested"
    /\ Control # "failure-after-prepare-is-rolled-back"
    /\ \/ /\ log' = log
          /\ ptgt' = ptgt
          /\ shared' = shared
          /\ cache' = cache
       \/ /\ log' = "prepared"
          /\ ptgt' = stage
          /\ shared' = Nx(shared)
          /\ cache' = Set(cache, p, Nx(shared))
    /\ pc' = Set(pc, p, "told")
    /\ fence' = TRUE
    /\ UNCHANGED <<adm, req, eligible, lockHolder, owner, trusted, count, gen, a, b,
                   stage, mygen, reader, diedIn, counts>>

\* Under Callbacks the store calls back into the caller once the effects are done, inside
\* the operation and under exclusion: a step of the holder that waits on nothing and
\* changes nothing but where the holder is. Each control breaks one clause of that rule.
Observe(p) ==
    /\ Callbacks /\ pc[p] = "wrote"
    /\ Control # "callback-never-returns"
    /\ Control = "callback-waits-for-exclusion" => lockHolder = None
    /\ pc' = Set(pc, p, "observed")
    /\ IF Control = "callback-changes-the-store"
          THEN /\ a' = Nx(a)
               /\ UNCHANGED <<adm, req, eligible, coord, b, shared, log, stage, ptgt,
                              handle, reader, diedIn, counts>>
          ELSE UNCHANGED <<adm, req, eligible, coord, store, handle, reader, diedIn, counts>>

\* The closing raise is prepared, then published.
BeginClose(p) ==
    /\ pc[p] = IF Callbacks THEN "observed" ELSE "wrote"
    /\ pc' = Set(pc, p, "closing")
    /\ UNCHANGED <<adm, req, eligible, coord, store, handle, reader, diedIn, counts>>

EndClose(p) ==
    /\ pc[p] = "closing"
    /\ count' = Nx(count)
    /\ pc' = Set(pc, p, "closed")
    /\ scache' = Cleared
    /\ UNCHANGED <<adm, req, eligible, lockHolder, owner, fence, trusted, gen, store,
                   handle, rphase, rbefore, rseenA, rseenB, rpart, disc, diedIn,
                   counts>>

\* A failed closing publication leaves the count odd and sets the fence, so
\* recovery, which republishes it, is required.
CloseFails(p) ==
    /\ pc[p] = "closing"
    /\ pc' = Set(pc, p, "told")
    /\ fence' = TRUE
    /\ UNCHANGED <<adm, req, eligible, lockHolder, owner, trusted, count, gen, store,
                   handle, reader, diedIn, counts>>

\* A clean release clears the ownership record while still holding the lock,
\* then drops the lock. A holder with an unresolved operation cannot release
\* cleanly.
Release(p) ==
    /\ pc[p] \in {"held", "closed", "told"}
    /\ IF Control = "unlock-before-clear"
          THEN /\ lockHolder' = None
               /\ owner' = owner
               /\ pc' = Set(pc, p, "unlocked")
          ELSE /\ lockHolder' = lockHolder
               /\ owner' = None
               /\ pc' = Set(pc, p, "releasing")
    /\ UNCHANGED <<adm, req, eligible, fence, trusted, count, gen, store, handle,
                   reader, diedIn, counts>>

\* An upgrader that lets go of exclusion has finished with its handle too: the
\* upgrade is served if the layer reached its target and refused otherwise, and
\* a handle whose upgrade was not completed is left detached.
Unlock(p) ==
    /\ pc[p] = "releasing"
    /\ lockHolder' = None
    /\ IF att[p] = "uBody"
          THEN /\ lph[p] = "none"
               /\ att' = Set(att, p, "off")
               /\ pc' = Set(pc, p, IF Completed(p) THEN "exited" ELSE "detached")
               /\ areq' = Set(areq, p, IF Completed(p) THEN "served" ELSE "refused")
               /\ rtier' = Set(rtier, p, FALSE)
               /\ UNCHANGED <<recorded, lease, reg>>
          ELSE /\ pc' = Set(pc, p, "idle")
               /\ UNCHANGED adm
    /\ UNCHANGED <<req, eligible, owner, fence, trusted, count, gen, store,
                   handle, reader, diedIn, counts>>

\* Only under Control = "unlock-before-clear": removes whatever record is
\* there, which may by now be another process's.
ClearRecord(p) ==
    /\ pc[p] = "unlocked"
    /\ owner' = None
    /\ pc' = Set(pc, p, "idle")
    /\ UNCHANGED <<adm, req, eligible, lockHolder, fence, trusted, count, gen, store,
                   handle, reader, diedIn, counts>>

-----------------------------------------------------------------------------
(* Transactions (DESIGN-NOTES "Counters"; src/store.rs recover). Under Txn  *)
(* the holder's operation is a transaction over the commit log.            *)

\* Each record the holder appends is durable when the step ends, and changes
\* what a handle may have cached of the log, so the holder updates its own.
Logged(p) ==
    /\ shared' = Nx(shared)
    /\ cache' = Set(cache, p, Nx(shared))

\* The begin record announces the transaction before it ingests anything, so
\* that anything it stages can be found and swept.
Begin(p) ==
    /\ Logs /\ pc[p] = "open" /\ att[p] # "uBody"
    /\ log' = "begun"
    /\ pc' = Set(pc, p, "begun")
    /\ Logged(p)
    /\ UNCHANGED <<adm, req, eligible, coord, a, b, stage, ptgt, mygen, reader,
                   diedIn, counts>>

\* Ingest stages every body the transaction writes, durably. A transaction
\* moves the cells from a settled store to the next version.
Ingest(p) ==
    /\ pc[p] = "begun"
    /\ stage' = Nx(a)
    /\ pc' = Set(pc, p, "ingested")
    /\ UNCHANGED <<adm, req, eligible, coord, a, b, shared, log, ptgt, handle,
                   reader, diedIn, counts>>

\* Where the prepare record is written: after ingest, or at another point under
\* a control.
PrepareFrom ==
    IF Control = "apply-before-prepare" THEN "applied"
    ELSE IF Control = "prepare-before-ingest" THEN "begun"
    ELSE "ingested"

\* The prepare record carries the swap set and is the commit point: once it is
\* in the log, the transaction is rolled forward and never back.
Prepare(p) ==
    /\ pc[p] = PrepareFrom
    /\ log' = "prepared"
    /\ ptgt' = IF stage # 0 THEN stage ELSE Nx(a)
    /\ pc' = Set(pc, p, IF Control = "apply-before-prepare" THEN "applied"
                        ELSE "prepared")
    /\ Logged(p)
    /\ UNCHANGED <<adm, req, eligible, coord, a, b, stage, mygen, reader, diedIn,
                   counts>>

\* A failure before the prepare record is written is rolled back: nothing in the
\* namespace has changed, so the staging is swept, the log reset, and the
\* operation ends without an effect.
AbortStates ==
    IF Control = "failure-after-prepare-is-rolled-back"
       THEN {"begun", "ingested", "prepared", "writing", "applied"}
       ELSE {"begun", "ingested"}

Abort(p) ==
    /\ Logs /\ pc[p] \in AbortStates
    /\ stage' = 0
    /\ log' = "empty"
    /\ ptgt' = 0
    /\ pc' = Set(pc, p, "wrote")
    /\ Logged(p)
    /\ UNCHANGED <<adm, req, eligible, coord, a, b, mygen, reader, diedIn, counts>>

\* The end record follows the last swap. Under Control = "end-before-the-swaps"
\* it may be written before every swap has landed.
End(p) ==
    /\ Logs /\ log = "prepared"
    /\ \/ pc[p] = "applied"
       \/ (Control = "end-before-the-swaps" /\ pc[p] \in {"prepared", "writing"})
    /\ log' = "ended"
    /\ ptgt' = 0
    /\ pc' = Set(pc, p, "ended")
    /\ Logged(p)
    /\ UNCHANGED <<adm, req, eligible, coord, a, b, stage, mygen, reader, diedIn,
                   counts>>

\* The sweep removes the transaction's staging.
Sweep(p) ==
    /\ Logs /\ pc[p] = "ended"
    /\ stage' = 0
    /\ pc' = Set(pc, p, "wrote")
    /\ UNCHANGED <<adm, req, eligible, coord, a, b, shared, log, ptgt, handle,
                   reader, diedIn, counts>>

\* The commit log is reset by a holder with nothing in flight, and only once
\* everything in it has ended. A reset never discards unresolved work.
QuiesceStates ==
    {"held", "closed"}
    \cup (IF Control = "reset-during-a-transaction"
             THEN {"begun", "ingested", "prepared", "writing", "applied"}
             ELSE {})
    \cup (IF Control = "reset-while-recovery-required" THEN {"told"} ELSE {})

Quiesce(p) ==
    /\ Logs /\ pc[p] \in QuiesceStates /\ log # "empty"
    /\ (pc[p] \in {"held", "closed"} => (log = "ended" /\ stage = 0))
    /\ log' = "empty"
    /\ ptgt' = 0
    /\ Logged(p)
    /\ UNCHANGED <<adm, pc, req, eligible, coord, a, b, stage, mygen, reader,
                   diedIn, counts>>

-----------------------------------------------------------------------------
(* Reads (DESIGN-NOTES "Counters"). A reader holds nothing.                *)

\* The reader's last sample ends the read.
FinishRead(p, v) ==
    /\ rphase' = Set(rphase, p, "none")
    /\ rbefore' = Set(rbefore, p, NoVal)
    /\ rseenA' = Set(rseenA, p, NoVal)
    /\ rseenB' = Set(rseenB, p, NoVal)
    /\ rpart' = Set(rpart, p, NoVal)
    /\ scache' = Set(scache, p, IF Control = "assembled-sample" THEN NoVal
                                ELSE v)
    /\ UNCHANGED disc

\* The first sample, in one step; as two parts, high half first, under
\* Control = "assembled-sample".
ReadBefore(p) ==
    /\ pc[p] = "idle" /\ rphase[p] = "none" /\ Room(reads, MaxReads)
    /\ reads' = Tick(reads)
    /\ IF Control = "assembled-sample"
          THEN /\ rpart' = Set(rpart, p, count \div 2)
               /\ rphase' = Set(rphase, p, "before2")
               /\ UNCHANGED <<adm, rbefore, scache>>
          ELSE /\ rbefore' = Set(rbefore, p, Sampled(p))
               /\ scache' = Set(scache, p, Sampled(p))
               /\ rphase' = Set(rphase, p, "obsA")
               /\ UNCHANGED rpart
    /\ UNCHANGED <<adm, pc, req, eligible, coord, store, handle, rseenA, rseenB,
                   disc, diedIn, requests, deaths, crashes, discoveries, upgrades>>

ReadBefore2(p) ==
    /\ pc[p] = "idle" /\ rphase[p] = "before2"
    /\ rbefore' = Set(rbefore, p, 2 * rpart[p] + (count % 2))
    /\ rphase' = Set(rphase, p, "obsA")
    /\ UNCHANGED <<adm, pc, req, eligible, coord, store, handle, rseenA, rseenB,
                   rpart, scache, disc, diedIn, counts>>

ObserveA(p) ==
    /\ pc[p] = "idle" /\ rphase[p] = "obsA"
    /\ rseenA' = Set(rseenA, p, a)
    /\ rphase' = Set(rphase, p, "obsB")
    /\ UNCHANGED <<adm, pc, req, eligible, coord, store, handle, rbefore, rseenB,
                   rpart, scache, disc, diedIn, counts>>

ObserveB(p) ==
    /\ pc[p] = "idle" /\ rphase[p] = "obsB"
    /\ rseenB' = Set(rseenB, p, b)
    /\ rphase' = Set(rphase, p, "after1")
    /\ UNCHANGED <<adm, pc, req, eligible, coord, store, handle, rbefore, rseenA,
                   rpart, scache, disc, diedIn, counts>>

\* The second sample ends the read: it is settled when the two samples are
\* equal and even, and contended otherwise.
ReadAfter(p) ==
    /\ pc[p] = "idle" /\ rphase[p] = "after1" /\ Control # "assembled-sample"
    /\ FinishRead(p, Sampled(p))
    /\ UNCHANGED <<adm, pc, req, eligible, coord, store, handle, diedIn, counts>>

ReadAfter1(p) ==
    /\ pc[p] = "idle" /\ rphase[p] = "after1" /\ Control = "assembled-sample"
    /\ rpart' = Set(rpart, p, count \div 2)
    /\ rphase' = Set(rphase, p, "after2")
    /\ UNCHANGED <<adm, pc, req, eligible, coord, store, handle, rbefore, rseenA,
                   rseenB, scache, disc, diedIn, counts>>

ReadAfter2(p) ==
    /\ pc[p] = "idle" /\ rphase[p] = "after2"
    /\ FinishRead(p, 2 * rpart[p] + (count % 2))
    /\ UNCHANGED <<adm, pc, req, eligible, coord, store, handle, diedIn, counts>>

\* A cached sample may be dropped at any time.
Invalidate(p) ==
    /\ scache[p] # NoVal
    /\ scache' = Set(scache, p, NoVal)
    /\ UNCHANGED <<adm, pc, req, eligible, coord, store, handle, rphase, rbefore,
                   rseenA, rseenB, rpart, disc, diedIn, counts>>

-----------------------------------------------------------------------------
(* The upgrade body (DESIGN-NOTES "Schemas and attachment"). Under UpSteps an  *)
(* upgrader holds exclusion for its body, and an interrupted upgrade is        *)
(* resumed under a recoverer's exclusion by the same steps.                    *)

\* The intent is recorded under exclusion, inside the count's bracket and before
\* any migration step: the layer is recorded as being upgraded, no step durable.
RecordIntent(p) ==
    /\ UpSteps > 0 /\ att[p] = "uBody" /\ pc[p] = "open"
    /\ upState' = IF SafeFor(p) THEN "safe" ELSE "unsafe"
    /\ ulayer' = UpL(p)
    /\ utgt' = UpTarget(p)
    /\ ufloor' = UpF(p)
    /\ ustep' = 0
    /\ pc' = Set(pc, p, "upBody")
    /\ UNCHANGED <<oldest, current, dfloor, rtier, att, areq, lease, reg, bystanders>>

\* A migration step is idempotent and takes what it needs from the recorded intent
\* and the progress durable state shows, so a resumer makes the rest. Under
\* Control = "step-before-the-intent" a step may be made before the intent.
UpStep(p) ==
    /\ UpSteps > 0 /\ ustep < UpSteps
    /\ \/ pc[p] \in {"upBody", "recUp"}
       \/ pc[p] = "open" /\ att[p] = "uBody" /\ Control = "step-before-the-intent"
    /\ ustep' = ustep + 1
    /\ UNCHANGED <<oldest, current, dfloor, upState, ulayer, utgt, ufloor, rtier, pc,
                   att, areq, lease, reg, bystanders>>

\* The layer's schema advances only after the last step, and an upgrade unsafe
\* for readers raises the oldest reader schema with it. Under Control =
\* "version-advanced-early" it may advance after the first step.
UpAdvance(p) ==
    /\ UpSteps > 0 /\ pc[p] \in {"upBody", "recUp"}
    /\ upState # "ordinary" /\ ~Advanced
    /\ \/ ustep = UpSteps
       \/ Control = "version-advanced-early" /\ ustep >= 1
    /\ current' = Set(current, ulayer, utgt)
    /\ dfloor' = Set(dfloor, ulayer, Max(dfloor[ulayer], ufloor))
    /\ oldest' = OldestAfterCompleting
    /\ UNCHANGED <<upState, ulayer, utgt, ufloor, ustep, rtier, pc, att, areq, lease,
                   reg, bystanders>>

\* The intent clears only once the schema has advanced, and takes its progress
\* with it. The upgrader's operation is then over; a resumer goes on to the rest
\* of recovery. Under Control = "interrupted-upgrade-rolled-back" a resumer clears
\* the intent without advancing the schema.
UpClear(p) ==
    /\ UpSteps > 0 /\ pc[p] \in {"upBody", "recUp"} /\ upState # "ordinary"
    /\ (Advanced \/ (pc[p] = "recUp" /\ Control = "interrupted-upgrade-rolled-back"))
    /\ upState' = "ordinary"
    /\ ustep' = 0
    /\ ulayer' = 0
    /\ utgt' = 0
    /\ ufloor' = 0
    /\ pc' = Set(pc, p, IF pc[p] = "upBody" THEN "wrote" ELSE "recEnd")
    /\ UNCHANGED <<oldest, current, dfloor, rtier, att, areq, lease, reg, bystanders>>

\* A failure in the body, or a cancellation there, cannot be rolled back: the
\* layer is partly migrated. It sets the fence and leaves the count odd and the
\* intent in place, and the upgrader is told recovery is required, which only a
\* handle that speaks the target schema can run.
UpFails(p) ==
    /\ UpSteps > 0 /\ pc[p] = "upBody"
    /\ pc' = Set(pc, p, "told")
    /\ fence' = TRUE
    /\ UNCHANGED <<adm, req, eligible, lockHolder, owner, trusted, count, gen, store,
                   handle, reader, diedIn, counts>>

-----------------------------------------------------------------------------
(* Recovery (DESIGN-NOTES "Exclusion and recovery", "Consequences"; src/store.rs *)
(* recover) and its discovery.                                             *)

\* The count a recovery publishes: even, and above every value a reader may
\* have sampled, so that no earlier even sample can compare equal to it.
RecoveredCount == IF Odd(Visible) THEN Nx(Visible) ELSE Visible

\* A holder told that recovery is required, or whose own transaction failed,
\* runs recovery under a capability ordinary writes do not have, from the log
\* alone. It starts at the swaps a prepared transaction names, and otherwise at
\* the end of the log; an unlogged write's recovery starts by making a store half
\* applied whole. Under Control = "recovery-rolls-back-a-prepared-
\* transaction" it never makes the swaps, under "count-published-before-the-
\* namespace-is-whole" it publishes the count first, and under "fence-cleared-
\* at-recovery-start" it clears the fence as it starts.
RecStart(p) ==
    /\ (Txn \/ UnloggedRecovery) /\ pc[p] = "told" /\ CanWrite(p) /\ att[p] # "uBody"
    /\ pc' = Set(pc, p,
                 IF Control = "count-published-before-the-namespace-is-whole"
                    THEN "recCount"
                 ELSE IF IntentStands THEN "recUp"
                 ELSE IF log = "prepared"
                         /\ Control # "recovery-rolls-back-a-prepared-transaction"
                    THEN "recSwap1"
                 ELSE IF ~Txn /\ Partial THEN "recWhole"
                 ELSE "recEnd")
    /\ fence' = IF Control = "fence-cleared-at-recovery-start" THEN FALSE ELSE fence
    /\ UNCHANGED <<adm, req, eligible, lockHolder, owner, trusted, count, gen, store,
                   handle, reader, diedIn, counts>>

RecSwap1(p) ==
    /\ pc[p] = "recSwap1" /\ log = "prepared"
    /\ a' = ptgt
    /\ pc' = Set(pc, p, "recSwap2")
    /\ UNCHANGED <<adm, req, eligible, coord, b, shared, log, stage, ptgt, handle,
                   reader, diedIn, counts>>

\* An unlogged write's half-applied store is made whole, forward or back: the
\* STRANDED-VOTE guard decides which, and both leave nothing half applied.
RecWhole(p) ==
    /\ pc[p] = "recWhole"
    /\ \/ /\ b' = a
          /\ UNCHANGED <<a, shared, cache>>
       \/ /\ a' = b
          /\ shared' = Pv(shared)
          /\ cache' = Set(cache, p, Pv(shared))
          /\ UNCHANGED b
    /\ pc' = Set(pc, p, "recEnd")
    /\ UNCHANGED <<adm, req, eligible, coord, log, stage, ptgt, mygen, reader,
                   diedIn, counts>>

RecSwap2(p) ==
    /\ pc[p] = "recSwap2" /\ log = "prepared"
    /\ b' = ptgt
    /\ pc' = Set(pc, p, "recEnd")
    /\ UNCHANGED <<adm, req, eligible, coord, a, shared, log, stage, ptgt, handle,
                   reader, diedIn, counts>>

\* A prepared transaction is ended; one that was only begun has its staging swept
\* before its log is reset; anything else is already resolved.
RecEnd(p) ==
    /\ pc[p] = "recEnd"
    /\ \/ /\ log = "prepared" /\ Control # "recovery-rolls-back-a-prepared-transaction"
          /\ log' = "ended"
          /\ ptgt' = 0
          /\ stage' = stage
          /\ pc' = Set(pc, p, "recSweep")
          /\ Logged(p)
       \/ /\ log = "prepared" /\ Control = "recovery-rolls-back-a-prepared-transaction"
          /\ log' = "empty"
          /\ ptgt' = 0
          /\ stage' = 0
          /\ pc' = Set(pc, p, "recSweep")
          /\ Logged(p)
       \/ /\ log = "begun"
          /\ stage' = 0
          /\ pc' = Set(pc, p, "recReset")
          /\ UNCHANGED <<log, ptgt, shared, cache>>
       \/ /\ log \in {"empty", "ended"}
          /\ pc' = Set(pc, p, "recSweep")
          /\ UNCHANGED <<log, ptgt, stage, shared, cache>>
    /\ UNCHANGED <<adm, req, eligible, coord, a, b, mygen, reader, diedIn, counts>>

RecReset(p) ==
    /\ pc[p] = "recReset" /\ log = "begun"
    /\ log' = "empty"
    /\ pc' = Set(pc, p, "recSweep")
    /\ Logged(p)
    /\ UNCHANGED <<adm, req, eligible, coord, a, b, stage, ptgt, mygen, reader,
                   diedIn, counts>>

RecSweep(p) ==
    /\ pc[p] = "recSweep"
    /\ stage' = 0
    /\ pc' = Set(pc, p,
                 IF Control = "count-published-before-the-namespace-is-whole"
                    THEN "recClear" ELSE "recCount")
    /\ UNCHANGED <<adm, req, eligible, coord, a, b, shared, log, ptgt, handle,
                   reader, diedIn, counts>>

\* The count is published even, together with the trust that only a completed
\* recovery can give it, and the cached samples of the old value dropped. A
\* resumed recovery finds the count even and republishes it unchanged. With the
\* store settled and trusted, the abandonment that required recovery is over.
RecCount(p) ==
    /\ pc[p] = "recCount"
    /\ count' = RecoveredCount
    /\ trusted' = TRUE
    /\ diedIn' = "none"
    /\ scache' = Cleared
    /\ pc' = Set(pc, p,
                 IF Control = "count-published-before-the-namespace-is-whole"
                    THEN (IF log = "prepared" THEN "recSwap1"
                          ELSE IF ~Txn /\ Partial THEN "recWhole"
                          ELSE "recEnd")
                    ELSE "recClear")
    /\ UNCHANGED <<adm, req, eligible, lockHolder, owner, fence, gen, store,
                   handle, rphase, rbefore, rseenA, rseenB, rpart, disc, counts>>

\* The last step: only now is the fence cleared.
RecClear(p) ==
    /\ pc[p] = "recClear"
    /\ fence' = FALSE
    /\ pc' = Set(pc, p, "closed")
    /\ UNCHANGED <<adm, req, eligible, lockHolder, owner, trusted, count, gen, store,
                   handle, reader, diedIn, counts>>

\* A recoverer stops between steps, holding exclusion and leaving the rest for
\* itself or the next.
RecStop(p) ==
    /\ pc[p] \in RecStates
    /\ pc' = Set(pc, p, "told")
    /\ UNCHANGED <<adm, req, eligible, coord, store, handle, reader, diedIn, counts>>

\* What discovery reports: recovery is required when the fence is set, when the
\* coordination state cannot be vouched for, or when a record names a holder
\* that is no longer there, which a lock that is free shows. A live holder's
\* record, beside a lock that is held, is not abandoned work.
Reported ==
    \/ fence /\ Control # "discovery-ignores-the-fence"
    \/ ~trusted /\ Control # "discovery-ignores-the-restart"
    \/ owner # None
       /\ (lockHolder = None \/ Control = "discovery-trusts-the-ownership-record")

\* Any attached handle can discover. It reads in one step and changes nothing but
\* its own report; under Control = "discovery-clears-the-fence" it also clears a
\* fence it finds set when nobody holds exclusion.
Discover(p) ==
    /\ pc[p] = "idle" /\ att[p] = "attached" /\ rphase[p] = "none"
    /\ Room(discoveries, MaxDiscoveries)
    /\ disc' = Set(disc, p, IF Reported THEN "required" ELSE "notRequired")
    /\ discoveries' = Tick(discoveries)
    /\ fence' = IF Control = "discovery-clears-the-fence" /\ lockHolder = None
                   THEN FALSE ELSE fence
    /\ UNCHANGED <<adm, pc, req, eligible, lockHolder, owner, trusted, count, gen,
                   store, handle, rphase, rbefore, rseenA, rseenB, rpart, scache,
                   diedIn, requests, deaths, crashes, reads, upgrades>>

=============================================================================
