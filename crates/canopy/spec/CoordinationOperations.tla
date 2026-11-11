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
    /\ gen' = gen + 1
    /\ mygen' = Set(mygen, p, gen + 1)
    /\ cache' = Set(cache, p,
                    IF cache[p] # NoVal
                          /\ (mygen[p] = gen \/ Control = "cache-always-kept")
                       THEN cache[p] ELSE shared)
    /\ pc' = Set(pc, p,
                 IF owner # None \/ fence
                       \/ (~trusted /\ Control # "ignore-restart")
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
    /\ count' = count + 1
    /\ pc' = Set(pc, p, "open")
    /\ scache' = Cleared
    /\ UNCHANGED <<adm, req, eligible, lockHolder, owner, fence, trusted, gen, store,
                   handle, rphase, rbefore, rseenA, rseenB, rpart, diedIn,
                   counts>>

\* A failed opening publication makes no effect and leaves the count as it was.
OpenFails(p) ==
    /\ pc[p] = "opening"
    /\ pc' = Set(pc, p, "closed")
    /\ UNCHANGED <<adm, req, eligible, coord, store, handle, reader, diedIn, counts>>

\* The write's first and last effects. The first changes shared, and the
\* holder's cache with it.
FirstEffect(p) ==
    /\ pc[p] = "open"
    /\ pc' = Set(pc, p, "writing")
    /\ a' = a + 1
    /\ shared' = shared + 1
    /\ cache' = Set(cache, p, shared + 1)
    /\ UNCHANGED <<adm, req, eligible, coord, b, mygen, reader, diedIn, counts>>

LastEffect(p) ==
    /\ pc[p] = "writing"
    /\ pc' = Set(pc, p, "wrote")
    /\ b' = b + 1
    /\ UNCHANGED <<adm, req, eligible, coord, a, shared, handle, reader, diedIn,
                   counts>>

\* An operation that finds nothing to change ends without an effect, and one
\* that fails after its first effect but can be rolled back undoes it; either
\* then publishes the closing raise, so the count is even again.
EndEmpty(p) ==
    /\ pc[p] = "open"
    /\ pc' = Set(pc, p, "wrote")
    /\ UNCHANGED <<adm, req, eligible, coord, store, handle, reader, diedIn, counts>>

Rollback(p) ==
    /\ pc[p] = "writing"
    /\ pc' = Set(pc, p, "wrote")
    /\ a' = b
    /\ shared' = shared - 1
    /\ cache' = Set(cache, p, shared - 1)
    /\ UNCHANGED <<adm, req, eligible, coord, b, mygen, reader, diedIn, counts>>

\* An operation that fails after its first effect and cannot be rolled back sets
\* the fence and leaves the count odd.
FailUnresolved(p) ==
    /\ pc[p] = "writing"
    /\ pc' = Set(pc, p, "told")
    /\ fence' = TRUE
    /\ UNCHANGED <<adm, req, eligible, lockHolder, owner, trusted, count, gen, store,
                   handle, reader, diedIn, counts>>

\* An operation nested under the hold changes nothing: the depth is tracked, so
\* an inner operation never restores even parity early.
EnterInner(p) ==
    /\ pc[p] = "writing"
    /\ pc' = Set(pc, p, "nested")
    /\ UNCHANGED <<adm, req, eligible, coord, store, handle, reader, diedIn, counts>>

LeaveInner(p) ==
    /\ pc[p] = "nested"
    /\ pc' = Set(pc, p, "writing")
    /\ count' = IF Control = "inner-restores-parity" THEN count + 1 ELSE count
    /\ scache' = IF Control = "inner-restores-parity" THEN Cleared ELSE scache
    /\ UNCHANGED <<adm, req, eligible, lockHolder, owner, fence, trusted, gen, store,
                   handle, rphase, rbefore, rseenA, rseenB, rpart, diedIn,
                   counts>>

\* The closing raise is prepared, then published.
BeginClose(p) ==
    /\ pc[p] = "wrote"
    /\ pc' = Set(pc, p, "closing")
    /\ UNCHANGED <<adm, req, eligible, coord, store, handle, reader, diedIn, counts>>

EndClose(p) ==
    /\ pc[p] = "closing"
    /\ count' = count + 1
    /\ pc' = Set(pc, p, "closed")
    /\ scache' = Cleared
    /\ UNCHANGED <<adm, req, eligible, lockHolder, owner, fence, trusted, gen, store,
                   handle, rphase, rbefore, rseenA, rseenB, rpart, diedIn,
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

Unlock(p) ==
    /\ pc[p] = "releasing"
    /\ lockHolder' = None
    /\ pc' = Set(pc, p, "idle")
    /\ UNCHANGED <<adm, req, eligible, owner, fence, trusted, count, gen, store,
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

\* The first sample, in one step; as two parts, high half first, under
\* Control = "assembled-sample".
ReadBefore(p) ==
    /\ pc[p] = "idle" /\ rphase[p] = "none" /\ reads < MaxReads
    /\ reads' = reads + 1
    /\ IF Control = "assembled-sample"
          THEN /\ rpart' = Set(rpart, p, count \div 2)
               /\ rphase' = Set(rphase, p, "before2")
               /\ UNCHANGED <<adm, rbefore, scache>>
          ELSE /\ rbefore' = Set(rbefore, p, Sampled(p))
               /\ scache' = Set(scache, p, Sampled(p))
               /\ rphase' = Set(rphase, p, "obsA")
               /\ UNCHANGED rpart
    /\ UNCHANGED <<adm, pc, req, eligible, coord, store, handle, rseenA, rseenB,
                   diedIn, requests, deaths, crashes>>

ReadBefore2(p) ==
    /\ pc[p] = "idle" /\ rphase[p] = "before2"
    /\ rbefore' = Set(rbefore, p, 2 * rpart[p] + (count % 2))
    /\ rphase' = Set(rphase, p, "obsA")
    /\ UNCHANGED <<adm, pc, req, eligible, coord, store, handle, rseenA, rseenB,
                   rpart, scache, diedIn, counts>>

ObserveA(p) ==
    /\ pc[p] = "idle" /\ rphase[p] = "obsA"
    /\ rseenA' = Set(rseenA, p, a)
    /\ rphase' = Set(rphase, p, "obsB")
    /\ UNCHANGED <<adm, pc, req, eligible, coord, store, handle, rbefore, rseenB,
                   rpart, scache, diedIn, counts>>

ObserveB(p) ==
    /\ pc[p] = "idle" /\ rphase[p] = "obsB"
    /\ rseenB' = Set(rseenB, p, b)
    /\ rphase' = Set(rphase, p, "after1")
    /\ UNCHANGED <<adm, pc, req, eligible, coord, store, handle, rbefore, rseenA,
                   rpart, scache, diedIn, counts>>

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
                   rseenB, scache, diedIn, counts>>

ReadAfter2(p) ==
    /\ pc[p] = "idle" /\ rphase[p] = "after2"
    /\ FinishRead(p, 2 * rpart[p] + (count % 2))
    /\ UNCHANGED <<adm, pc, req, eligible, coord, store, handle, diedIn, counts>>

\* A cached sample may be dropped at any time.
Invalidate(p) ==
    /\ scache[p] # NoVal
    /\ scache' = Set(scache, p, NoVal)
    /\ UNCHANGED <<adm, pc, req, eligible, coord, store, handle, rphase, rbefore,
                   rseenA, rseenB, rpart, diedIn, counts>>

=============================================================================
