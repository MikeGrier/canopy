\* Copyright (c) Michael Grier.
\* Split from Coordination.tla at 70668aa.
----------------------- MODULE CoordinationAttachment -----------------------
EXTENDS CoordinationState

-----------------------------------------------------------------------------
(* Attachment and requests.                                                *)

\* A clean detach, or the drop of a detached handle, once none of its clones
\* has an operation in flight and no listing of its own is running. A handle that
\* is attached unregisters before it lets go of its tiers.
Exit(p) ==
    /\ pc[p] \in {"idle", "detached"} /\ rphase[p] = "none" /\ lease[p] = 0
    /\ lph[p] = "none" /\ rst[p] \in {"out", "in", "unreg"}
    /\ IF pc[p] = "idle" /\ RegistryOn
          THEN /\ pc' = Set(pc, p, "detaching")
               /\ att' = Set(att, p, "unregistering")
               /\ UNCHANGED <<handle, reader>>
          ELSE /\ pc' = Set(pc, p, "exited")
               /\ att' = Set(att, p, "off")
               /\ Forget(p)
    /\ UNCHANGED <<areq, lease, schema, req, eligible, coord, store, diedIn,
                   counts, reg>>

\* EXTERNAL: the admission rules stop admitting p's handle.
Disqualify(p) ==
    /\ Escapes
    /\ pc[p] \in {"idle", "waiting"} /\ eligible[p]
    /\ eligible' = Set(eligible, p, FALSE)
    /\ UNCHANGED <<adm, pc, req, coord, store, handle, reader, diedIn, counts>>

Request(p) ==
    /\ CanWrite(p)
    /\ pc[p] = "idle" /\ rphase[p] = "none" /\ Room(requests, MaxRequests)
    /\ pc' = Set(pc, p, "waiting")
    /\ req' = Set(req, p, "pending")
    /\ requests' = Tick(requests)
    /\ UNCHANGED <<adm, eligible, coord, store, handle, reader, diedIn, deaths,
                   crashes, reads, discoveries, upgrades, woken>>

\* The waiter gives up.
Withdraw(p) ==
    /\ Escapes
    /\ pc[p] = "waiting"
    /\ pc' = Set(pc, p, "idle")
    /\ req' = Set(req, p, "withdrawn")
    /\ UNCHANGED <<adm, eligible, coord, store, handle, reader, diedIn, counts>>

\* A request that is no longer eligible is refused rather than served.
Refuse(p) ==
    /\ Escapes
    /\ pc[p] = "waiting" /\ ~eligible[p]
    /\ pc' = Set(pc, p, "idle")
    /\ req' = Set(req, p, "refused")
    /\ UNCHANGED <<adm, eligible, coord, store, handle, reader, diedIn, counts>>

-----------------------------------------------------------------------------
(* The admission gate and the attachment tiers (DESIGN-NOTES "Schemas and *)
(* attachment"). The gate and each tier are locks that ordinary handles    *)
(* share and an upgrade takes exclusively. Who holds each follows from att, *)
(* so a process's death releases all of them.                              *)

\* An ordinary handle passes the gate shared, takes the reader tier, drops the
\* gate before it waits for anything else, takes the writer tier if it may
\* write, and only then is checked, so an upgrade cannot complete between the
\* check and the attachment. A detached handle attaches again the same way.
BeginAttach(p) ==
    /\ pc[p] \in {"new", "detached"}
    /\ rst[p] = "out"
    /\ pc' = Set(pc, p, "attaching")
    /\ att' = Set(att, p, "wGate")
    /\ areq' = Set(areq, p, "pending")
    /\ UNCHANGED <<schema, rest>>

TakeGateS(p) ==
    /\ att[p] = "wGate" /\ ~GateXHeld
    /\ att' = Set(att, p, "hasGate")
    /\ UNCHANGED <<pc, areq, schema, rest>>

TakeReaderS(p) ==
    /\ att[p] = "hasGate" /\ ~ReaderXHeld
    /\ att' = Set(att, p, "gateRd")
    /\ UNCHANGED <<pc, areq, schema, rest>>

DropGate(p) ==
    /\ att[p] = "gateRd"
    /\ ~(CanWrite(p) /\ Control = "gate-held-through-writer-tier")
    /\ att' = Set(att, p, IF CanWrite(p) THEN "rd" ELSE "tiers")
    /\ UNCHANGED <<pc, areq, schema, rest>>

\* Under Control = "gate-held-through-writer-tier" a handle that may write keeps
\* the gate while it waits for the writer tier.
KeepGate(p) ==
    /\ att[p] = "gateRd" /\ CanWrite(p)
    /\ Control = "gate-held-through-writer-tier"
    /\ att' = Set(att, p, "gateWr")
    /\ UNCHANGED <<pc, areq, schema, rest>>

\* Under Control = "check-before-writer-tier" the check is made before the
\* writer tier is waited for, and not again.
CheckEarly(p) ==
    /\ Control = "check-before-writer-tier" /\ att[p] = "rd"
    /\ IF Admissible(p)
          THEN /\ att' = Set(att, p, "rdChecked")
               /\ UNCHANGED <<pc, areq>>
          ELSE /\ lph[p] = "none"
               /\ att' = Set(att, p, "off")
               /\ pc' = Set(pc, p, "exited")
               /\ areq' = Set(areq, p, "refused")
    /\ UNCHANGED <<schema, rest>>

TakeWriterS(p) ==
    /\ \/ att[p] = "rd" /\ Control # "check-before-writer-tier"
       \/ att[p] \in {"gateWr", "rdChecked"}
    /\ ~WriterXHeld
    /\ IF att[p] = "rdChecked"
          THEN /\ att' = Set(att, p, IF RegistryOn THEN "registering" ELSE "attached")
               /\ pc' = IF RegistryOn THEN pc ELSE Set(pc, p, "idle")
               /\ areq' = IF RegistryOn THEN areq ELSE Set(areq, p, "served")
          ELSE /\ att' = Set(att, p, "tiers")
               /\ UNCHANGED <<pc, areq>>
    /\ UNCHANGED <<schema, rest>>

\* The check, under the shared tiers it needs: a handle the rules do not admit
\* is refused and releases them. One that is admitted goes on to register, and
\* its attach returns once it has.
Admit(p) ==
    /\ att[p] = "tiers"
    /\ IF Admissible(p)
          THEN /\ att' = Set(att, p, IF RegistryOn THEN "registering" ELSE "attached")
               /\ pc' = IF RegistryOn THEN pc ELSE Set(pc, p, "idle")
               /\ areq' = IF RegistryOn THEN areq ELSE Set(areq, p, "served")
          ELSE /\ lph[p] = "none"
               /\ att' = Set(att, p, "off")
               /\ pc' = Set(pc, p, "exited")
               /\ areq' = Set(areq, p, "refused")
    /\ UNCHANGED <<schema, rest>>

\* A handle that is waiting gives up, releasing whatever it holds. A conversion
\* that gives up after its drain leaves the handle detached, at once and with
\* nothing left to wait for.
GiveUp(p) ==
    /\ Escapes /\ lph[p] = "none"
    /\ \/ att[p] \in {"wGate", "hasGate", "gateWr", "rd", "rdChecked"}
       \/ att[p] \in {"uWGate", "uGate", "uRd", "uWr"} /\ pc[p] = "converting"
    /\ att' = Set(att, p, "off")
    /\ pc' = Set(pc, p,
                 IF pc[p] # "converting" THEN "exited"
                 ELSE IF Control = "cancelled-conversion-stays-usable"
                         THEN "idle" ELSE "detached")
    /\ areq' = Set(areq, p, "withdrawn")
    /\ rtier' = Set(rtier, p, FALSE)
    /\ UNCHANGED <<recorded, rest>>

\* A clone of an attached handle starts an operation, which holds a lease on the
\* attachment until it ends. A handle that is draining or converting refuses it.
BeginOp(p) ==
    /\ \/ att[p] = "attached"
       \/ att[p] = "draining" /\ Control = "clone-starts-while-draining"
       \/ att[p] = "uBody" /\ Control = "clone-starts-during-the-body"
    /\ lease[p] < MaxLeases
    /\ lease' = Set(lease, p, lease[p] + 1)
    /\ UNCHANGED <<pc, att, areq, schema, reg, bystanders>>

EndOp(p) ==
    /\ lease[p] > 0
    /\ lease' = Set(lease, p, lease[p] - 1)
    /\ UNCHANGED <<pc, att, areq, schema, reg, bystanders>>

\* An upgrader first drains: new operations on every clone are refused and it
\* waits for the leases to end while it still holds its tiers. Called from inside
\* an operation it would wait for that operation, which cannot end while the
\* call is blocked, so the call is refused; under Control =
\* "upgrade-from-inside-an-operation" it starts and waits for itself.
StartUpgrade(p) ==
    /\ Upgrader(p) /\ pc[p] = "idle" /\ att[p] = "attached"
    /\ (rphase[p] = "none" \/ Control = "upgrade-from-inside-an-operation")
    /\ (UpSteps = 0 \/ upgrades < MaxUpgrades)
    /\ upgrades' = IF UpSteps > 0 THEN upgrades + 1 ELSE upgrades
    /\ pc' = Set(pc, p, "converting")
    /\ att' = Set(att, p, "draining")
    /\ areq' = Set(areq, p, "pending")
    /\ UNCHANGED <<schema, lease, reg, req, eligible, coord, store, handle, reader,
                   diedIn, requests, deaths, crashes, reads, discoveries, woken>>

RefuseUpgrade(p) ==
    /\ Upgrader(p) /\ att[p] = "attached" /\ InsideOperation(p)
    /\ areq' = Set(areq, p, "refused")
    /\ UNCHANGED <<pc, att, schema, rest>>

\* Once no lease is outstanding the drain ends and the conversion begins: the
\* upgrader releases its shared tiers before it asks for the gate exclusively,
\* so it never waits for the gate while holding a tier. Under Control =
\* "upgrader-keeps-tiers" it keeps them until it has the gate, and under
\* "drain-ignores-leases" it does not wait for the leases. The handle leaves the
\* registry first, since its entry follows its attachment and not its life, which
\* Control = "entry-follows-the-handles-life" reverses.
DrainDone(p) ==
    /\ att[p] = "draining"
    /\ rst[p] \in {"out", "in", "unreg"}
    /\ (Leases(p) = 0 \/ Control = "drain-ignores-leases")
    /\ att' = Set(att, p,
                  IF RegistryOn /\ Control # "entry-follows-the-handles-life"
                     THEN "unregistering"
                     ELSE IF Control = "upgrader-keeps-tiers" THEN "uWGateT"
                                                              ELSE "uWGate")
    /\ UNCHANGED <<pc, areq, schema, rest>>

\* A drain is cancelled by its caller; the handle stays attached and usable.
CancelDrain(p) ==
    /\ Escapes /\ att[p] = "draining"
    /\ IF Control = "cancelled-drain-detaches"
          THEN /\ att' = Set(att, p, "off")
               /\ pc' = Set(pc, p, "detached")
          ELSE /\ att' = Set(att, p, "attached")
               /\ pc' = Set(pc, p, "idle")
    /\ areq' = Set(areq, p, "withdrawn")
    /\ UNCHANGED <<schema, rest>>

\* Under Control = "upgrader-holds-exclusion" an upgrader starts while it holds
\* writer exclusion, without draining.
StartUpgradeHolding(p) ==
    /\ Control = "upgrader-holds-exclusion"
    /\ Upgrader(p) /\ pc[p] = "held" /\ att[p] = "attached"
    /\ pc' = Set(pc, p, "convertingX")
    /\ att' = Set(att, p, "uWGate")
    /\ areq' = Set(areq, p, "pending")
    /\ UNCHANGED <<schema, rest>>

TakeGateX(p) ==
    /\ att[p] \in {"uWGate", "uWGateT"}
    /\ ~GateSHeld /\ ~GateXHeld
    /\ att' = Set(att, p, "uGate")
    /\ UNCHANGED <<pc, areq, schema, rest>>

\* The exclusive tiers follow the order of the gate, then the reader tier, then
\* the writer tier: an upgrade that drains readers takes the reader tier first.
\* Under Control = "unsafe-writer-tier-first" it takes the writer tier first.
TakeFirstTier(p) ==
    /\ att[p] = "uGate"
    /\ LET rt == DecideReaderTier(p) IN
       /\ rtier' = Set(rtier, p, rt)
       /\ IF rt /\ Control # "unsafe-writer-tier-first"
             THEN /\ ~ReaderSHeld /\ ~ReaderXHeld
                  /\ att' = Set(att, p, "uRd")
             ELSE /\ ~WriterSHeld /\ ~WriterXHeld
                  /\ att' = Set(att, p, IF rt THEN "uWr" ELSE "uTiers")
    /\ UNCHANGED <<pc, areq, recorded, rest>>

TakeSecondTier(p) ==
    /\ \/ att[p] = "uRd" /\ ~WriterSHeld /\ ~WriterXHeld
       \/ att[p] = "uWr" /\ ~ReaderSHeld /\ ~ReaderXHeld
    /\ att' = Set(att, p, "uTiers")
    /\ UNCHANGED <<pc, areq, schema, rest>>

\* Holding its tiers, an upgrader checks the schemas again. If they are as it
\* found them and no upgrade is under way, the upgrade begins; one that leaves
\* readers attached lets the gate reopen to them. Without a body (UpSteps = 0)
\* the upgrade is recorded as in progress at once. With one, the upgrader takes
\* writer exclusion, which no one else can hold while it holds the writer tier,
\* and goes on as a holder; the intent is recorded under it.
BeginUpgrade(p) ==
    /\ att[p] = "uTiers"
    /\ current[UpL(p)] = SpeaksOf[p][UpL(p)] /\ upState = "ordinary"
    /\ att' = Set(att, p, "uBody")
    /\ IF UpSteps = 0
          THEN /\ upState' = IF SafeFor(p) THEN "safe" ELSE "unsafe"
               /\ ulayer' = UpL(p)
               /\ utgt' = UpTarget(p)
               /\ ufloor' = UpF(p)
               /\ UNCHANGED <<oldest, current, dfloor, ustep, rtier, pc, areq, rest>>
          ELSE /\ lockHolder' = p
               /\ pc' = Set(pc, p, "locked")
               /\ UNCHANGED <<schema, areq, lease, reg, req, eligible, owner, fence,
                              trusted, count, gen, store, handle, reader, diedIn,
                              counts>>

\* Otherwise it gives up, and its handle is left detached.
UpgradeMoved(p) ==
    /\ att[p] = "uTiers"
    /\ ~(current[UpL(p)] = SpeaksOf[p][UpL(p)] /\ upState = "ordinary")
    /\ att' = Set(att, p, "off")
    /\ pc' = Set(pc, p, IF pc[p] = "convertingX" THEN "held" ELSE "detached")
    /\ areq' = Set(areq, p, "refused")
    /\ rtier' = Set(rtier, p, FALSE)
    /\ UNCHANGED <<recorded, rest>>

\* The upgrade completes: the layer moves to its target, the data's floor rises to
\* the upgrade's, and an upgrade unsafe for readers raises the oldest reader schema
\* to that floor with it.
EndUpgrade(p) ==
    /\ UpSteps = 0
    /\ att[p] = "uBody"
    /\ (pc[p] = "convertingX" \/ lph[p] = "none")
    /\ upState' = "ordinary"
    /\ ustep' = ustep
    /\ current' = Set(current, ulayer, utgt)
    /\ dfloor' = Set(dfloor, ulayer, Max(dfloor[ulayer], ufloor))
    /\ oldest' = OldestAfterCompleting
    /\ ulayer' = 0
    /\ utgt' = 0
    /\ ufloor' = 0
    /\ rtier' = Set(rtier, p, FALSE)
    /\ att' = Set(att, p, "off")
    /\ pc' = Set(pc, p, IF pc[p] = "convertingX" THEN "held" ELSE "exited")
    /\ areq' = Set(areq, p, "served")
    /\ UNCHANGED rest

-----------------------------------------------------------------------------
(* The registry of attached handles (DESIGN-NOTES "A registry lists the   *)
(* attached handles"). Each entry has a claim lock and a liveness lock,    *)
(* both OS locks that a death releases. Every change to an entry and every *)
(* inspection of one is made under its claim lock, which is only ever     *)
(* tried, never waited for, and a handle keeps its entry's liveness lock   *)
(* for as long as it stays attached.                                       *)

Unclaim(s, p) == IF cl[s] = p THEN Set(cl, s, None) ELSE cl

\* After its check a handle writes an entry before its attach returns. It tries
\* the entries one at a time: it claims one without waiting, tries the liveness
\* lock exclusively without waiting, which succeeds only if no handle holds the
\* entry, writes its token, and lets go of the claim lock last. Holding the claim
\* lock from before the liveness lock until after the token is written is what
\* keeps a listing from seeing a new owner's lock beside an old owner's token.
RegStart(p) ==
    /\ att[p] = "registering" /\ rst[p] = "out"
    /\ rst' = Set(rst, p, "scan")
    /\ rscan' = Set(rscan, p, Slots)
    /\ UNCHANGED <<core, rlock, rdata, ovf, rcur, rlist>>

\* A claim lock that is held moves the handle on to the next entry.
RegClaim(p, s) ==
    /\ rst[p] = "scan" /\ s \in rscan[p]
    /\ rscan' = Set(rscan, p, rscan[p] \ {s})
    /\ IF cl[s] = None \/ Control = "register-without-claim"
          THEN /\ cl' = IF Control = "register-without-claim" THEN cl
                                                             ELSE Set(cl, s, p)
               /\ rst' = Set(rst, p, "claimed")
               /\ rcur' = Set(rcur, p, s)
          ELSE UNCHANGED <<cl, rst, rcur>>
    /\ UNCHANGED <<core, ll, rdata, ovf, rlist>>

\* A liveness lock that is held means a live handle owns the entry, and the
\* handle moves on. Under Control = "reuse-without-liveness-probe" it takes the
\* entry regardless, and under "register-releases-claim-early" it lets go of the
\* claim lock before it has written.
RegLive(p) ==
    /\ rst[p] = "claimed"
    /\ IF ll[rcur[p]] = None \/ Control = "reuse-without-liveness-probe"
          THEN /\ ll' = IF ll[rcur[p]] = None THEN Set(ll, rcur[p], p) ELSE ll
               /\ rst' = Set(rst, p, "locked")
               /\ cl' = IF Control = "register-releases-claim-early"
                           THEN Unclaim(rcur[p], p) ELSE cl
               /\ UNCHANGED rcur
          ELSE /\ cl' = Unclaim(rcur[p], p)
               /\ rst' = Set(rst, p, "scan")
               /\ rcur' = Set(rcur, p, 0)
               /\ UNCHANGED ll
    /\ UNCHANGED <<core, rdata, ovf, rscan, rlist>>

RegWrite(p) ==
    /\ rst[p] = "locked"
    /\ etok' = Set(etok, rcur[p], p)
    /\ edr' = Set(edr, rcur[p], FALSE)
    /\ rst' = Set(rst, p, "written")
    /\ UNCHANGED <<core, rlock, ovf, rscan, rcur, rlist>>

RegRelease(p) ==
    /\ rst[p] = "written"
    /\ cl' = Unclaim(rcur[p], p)
    /\ rst' = Set(rst, p, "in")
    /\ att' = Set(att, p, "attached")
    /\ pc' = Set(pc, p, "idle")
    /\ areq' = Set(areq, p, "served")
    /\ UNCHANGED <<lease, schema, bystanders, ll, rdata, ovf, rscan, rcur, rlist>>

\* A handle that found every entry held attaches without one, and the registry
\* records that some handle is missing from it.
RegOverflow(p) ==
    /\ rst[p] = "scan" /\ rscan[p] = {}
    /\ ovf' = TRUE
    /\ rst' = Set(rst, p, "unreg")
    /\ att' = Set(att, p, "attached")
    /\ pc' = Set(pc, p, "idle")
    /\ areq' = Set(areq, p, "served")
    /\ UNCHANGED <<lease, schema, bystanders, rlock, rdata, rscan, rcur, rlist>>

\* An owner updates its entry when its draining state changes, under the claim
\* lock; if the claim lock is held it does not wait and tries again later.
UpdTake(p) ==
    /\ rst[p] = "in" /\ att[p] \in {"attached", "draining"}
    /\ edr[rcur[p]] # (att[p] = "draining") /\ cl[rcur[p]] = None
    /\ cl' = Set(cl, rcur[p], p)
    /\ rst' = Set(rst, p, "upd")
    /\ UNCHANGED <<core, ll, rdata, ovf, rscan, rcur, rlist>>

UpdDone(p) ==
    /\ rst[p] = "upd"
    /\ edr' = Set(edr, rcur[p], att[p] = "draining")
    /\ cl' = Unclaim(rcur[p], p)
    /\ rst' = Set(rst, p, "in")
    /\ UNCHANGED <<core, ll, etok, ovf, rscan, rcur, rlist>>

\* A handle leaves the registry before it lets go of its tiers: it claims its
\* entry without waiting, clears it, and releases the liveness lock and then the
\* claim lock. If the claim lock is held it does not wait: it releases the
\* liveness lock alone, which leaves the entry to read as unused.
UnregNone(p) ==
    /\ att[p] = "unregistering" /\ rst[p] = "unreg"
    /\ rst' = Set(rst, p, "out")
    /\ UNCHANGED <<core, rlock, rdata, ovf, rscan, rcur, rlist>>

UnregClaim(p) ==
    /\ att[p] = "unregistering" /\ rst[p] = "in" /\ cl[rcur[p]] = None
    /\ cl' = Set(cl, rcur[p], p)
    /\ rst' = Set(rst, p, "uc")
    /\ UNCHANGED <<core, ll, rdata, ovf, rscan, rcur, rlist>>

UnregSkip(p) ==
    /\ att[p] = "unregistering" /\ rst[p] = "in" /\ cl[rcur[p]] # None
    /\ ll' = Set(ll, rcur[p], None)
    /\ rst' = Set(rst, p, "out")
    /\ rcur' = Set(rcur, p, 0)
    /\ UNCHANGED <<core, cl, rdata, ovf, rscan, rlist>>

UnregClear(p) ==
    /\ rst[p] = "uc"
    /\ etok' = Set(etok, rcur[p], None)
    /\ edr' = Set(edr, rcur[p], FALSE)
    /\ rst' = Set(rst, p, "ucl")
    /\ UNCHANGED <<core, rlock, ovf, rscan, rcur, rlist>>

UnregRelease(p) ==
    /\ rst[p] = "ucl"
    /\ ll' = Set(ll, rcur[p], None)
    /\ cl' = Unclaim(rcur[p], p)
    /\ rst' = Set(rst, p, "out")
    /\ rcur' = Set(rcur, p, 0)
    /\ UNCHANGED <<core, rdata, ovf, rscan, rlist>>

\* Out of the registry, the handle lets go of its tiers: a handle that is being
\* dropped ends, and an upgrader goes on to ask for the gate.
Detach(p) ==
    /\ att[p] = "unregistering" /\ rst[p] = "out"
    /\ IF pc[p] = "detaching"
          THEN /\ lph[p] = "none"
               /\ pc' = Set(pc, p, "exited")
               /\ att' = Set(att, p, "off")
               /\ Forget(p)
          ELSE /\ att' = Set(att, p, IF Control = "upgrader-keeps-tiers"
                                        THEN "uWGateT" ELSE "uWGate")
               /\ UNCHANGED <<pc, handle, reader>>
    /\ UNCHANGED <<areq, lease, schema, req, eligible, coord, store, diedIn,
                   counts, reg>>

\* A listing runs on a thread of its own, on a process that need not be
\* attached, and visits the entries in turn: it claims an entry without
\* waiting, reporting it as unknown if it cannot, reads its token, and tries
\* the liveness lock in shared mode, which is released at once, reporting the
\* entry live only if the attempt was refused. It says at the end whether it
\* may be incomplete.
ListStart(p) ==
    /\ RegistryOn /\ lph[p] = "none" /\ Room(lists, MaxListings)
    /\ pc[p] \notin {"exited", "dead"}
    /\ lph' = Set(lph, p, "next")
    /\ lcur' = Set(lcur, p, 1)
    /\ lunk' = Set(lunk, p, FALSE)
    /\ lfree' = Set(lfree, p, FALSE)
    /\ lists' = Tick(lists)
    /\ UNCHANGED <<core, rlock, rdata, ovf, rown, ltok, lrep, lres>>

\* Under Control = "list-without-claim" it does not claim the entry at all.
ListClaim(p) ==
    /\ lph[p] = "next" /\ lcur[p] <= NSlots
    /\ IF Control = "list-without-claim"
          THEN /\ lph' = Set(lph, p, "held")
               /\ UNCHANGED <<cl, lcur, lunk, lrep>>
          ELSE IF cl[lcur[p]] = None
          THEN /\ cl' = Set(cl, lcur[p], p)
               /\ lph' = Set(lph, p, "held")
               /\ UNCHANGED <<lcur, lunk, lrep>>
          ELSE /\ lrep' = Set(lrep, p, <<lcur[p], None, "unknown">>)
               /\ lunk' = Set(lunk, p, TRUE)
               /\ lcur' = Set(lcur, p, lcur[p] + 1)
               /\ UNCHANGED <<cl, lph>>
    /\ UNCHANGED <<core, ll, rdata, ovf, rown, ltok, lfree, lres, lists>>

ListRead(p) ==
    /\ lph[p] = "held"
    /\ ltok' = Set(ltok, p, etok[lcur[p]])
    /\ lph' = Set(lph, p, "read")
    /\ UNCHANGED <<core, rlock, rdata, ovf, rown, lcur, lrep, lunk, lfree, lres,
                   lists>>

ListProbe(p) ==
    /\ lph[p] = "read"
    /\ lrep' = Set(lrep, p,
                   <<lcur[p], ltok[p],
                     IF ltok[p] = None THEN "free"
                     ELSE IF ll[lcur[p]] = None THEN "dead" ELSE "live">>)
    /\ lfree' = Set(lfree, p, lfree[p] \/ ltok[p] = None \/ ll[lcur[p]] = None)
    /\ cl' = Unclaim(lcur[p], p)
    /\ lcur' = Set(lcur, p, lcur[p] + 1)
    /\ lph' = Set(lph, p, "next")
    /\ UNCHANGED <<core, ll, rdata, ovf, rown, ltok, lunk, lres, lists>>

\* The listing says it may be incomplete when it could not claim an entry or when
\* the registry records that a handle attached without one; under Control =
\* "listing-infers-from-fullness" it says so only when no entry was free.
ListDone(p) ==
    /\ lph[p] = "next" /\ lcur[p] > NSlots
    /\ lres' = Set(lres, p,
                   IF lunk[p] \/ (IF Control = "listing-infers-from-fullness"
                                     THEN ~lfree[p] ELSE ovf)
                      THEN "incomplete" ELSE "complete")
    /\ lph' = Set(lph, p, "none")
    /\ UNCHANGED <<core, rlock, rdata, ovf, rown, lcur, ltok, lrep, lunk, lfree,
                   lists>>

=============================================================================
