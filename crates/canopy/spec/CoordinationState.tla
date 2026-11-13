\* Copyright (c) Michael Grier.
\* Split from Coordination.tla at 70668aa.
------------------------- MODULE CoordinationState --------------------------
(***************************************************************************)
(* Writer exclusion across the processes that open one store root, and the *)
(* counters that let a process holding nothing tell a settled store from   *)
(* one a writer is changing, as the coordination contract specifies them   *)
(* (crates/canopy/DESIGN-NOTES.md, "Several processes may open one store", *)
(* "Exclusion and recovery", "Counters"). Processes interact only through  *)
(* the contract: nothing here knows how an implementation provides the     *)
(* lock, the ownership record or the count.                                *)
(*                                                                         *)
(* A process attaches, then repeatedly requests writer exclusion, holds    *)
(* it while it writes, and releases it, or reads while holding nothing; it *)
(* may withdraw a request, have its request refused, or die at any step. A *)
(* process is used once: attaching starts it and exiting or dying ends it, *)
(* and one that has not attached yet stands for a process started later,   *)
(* after a host restart. Attaching is itself a protocol (below).           *)
(*                                                                         *)
(* Writer exclusion is two things: an OS lock, released when its holder    *)
(* dies, and an ownership record in the store root, which names its        *)
(* holder and survives the holder's death. A clean release clears the      *)
(* record and then drops the lock, so an acquirer that finds a record      *)
(* under the lock knows its holder died holding exclusion. It decides that *)
(* from the record alone: the contract gives it no way to ask whether a    *)
(* process is alive. Acquiring is three steps, each of which a process can *)
(* die between: take the lock, set the fence if a record was left behind,  *)
(* then replace the record, which also advances the generation.            *)
(*                                                                         *)
(* A write is one logical operation on two cells, a and b, standing for a  *)
(* transaction's two swaps: its first effect changes a and its last        *)
(* changes b, so a # b is a store half applied. The modification count     *)
(* brackets the operation: the opening raise is published before the first *)
(* effect and the closing raise after the last. Nothing nests under a      *)
(* hold: no caller code that could call the store again runs under it. A   *)
(* publication is two steps, prepare and publish, and only the second is   *)
(* visible, so a writer that dies or fails between them leaves the         *)
(* previous value published. A write that fails after its first effect, or *)
(* whose closing publication fails, sets the fence and leaves the count    *)
(* odd. Only recovery (below) clears the fence or restores trust, so until *)
(* it has run every later acquirer is told recovery is required, and a     *)
(* told holder writes nothing.                                             *)
(*                                                                         *)
(* The generation advances on every acquisition. A holder relies on its    *)
(* cache of shared, durable state any holder may change, without reading   *)
(* it again, and keeps that cache only if the generation it found is the   *)
(* one it last advanced to; otherwise it reloads.                          *)
(*                                                                         *)
(* A reader takes a sample of the count, observes a and then b in two      *)
(* separate steps, takes a second sample, and classifies the read with     *)
(* Store::undisturbed: the samples are equal and even. A sample is one     *)
(* atomic read of the published value, and may be served from a cache that *)
(* is invalidated synchronously with every publication. State that cannot  *)
(* be vouched for reads as odd.                                            *)
(*                                                                         *)
(* A process's death loses only that process: the live filesystem stays    *)
(* as it was and the OS releases its lock. A host crash kills every        *)
(* process and leaves the coordination state untrusted, because nothing    *)
(* shows it was maintained across the restart; it may also lose the        *)
(* ownership record and the fence, and roll the count back over a writer   *)
(* that was mid-operation. Whether store data is durable is                *)
(* PointerPublication.tla's concern, so a crash here leaves it alone.      *)
(*                                                                         *)
(* Waiting. A request is an individual object that ends in exactly one of  *)
(* four outcomes: served, withdrawn, dead with its process, or refused as  *)
(* ineligible (req). There is no queue or ticket in the state: whenever    *)
(* the lock is free, acquisition is enabled for every eligible waiting     *)
(* process and the checker chooses among them, so every invariant is       *)
(* checked against every service order. "eligible" stands for the          *)
(* admission rules and only ever goes from true to false.                  *)
(*                                                                         *)
(* Attachment. A process is a reader, a writer, or an upgrader of one      *)
(* layer, and speaks a schema in each layer (Scenario). It attaches        *)
(* through an admission gate and two tiers, locks that ordinary handles    *)
(* hold shared and an upgrade holds exclusively, taken in the order gate,  *)
(* reader tier, writer tier, writer exclusion. A handle holds the gate     *)
(* only until it holds the reader tier, and no process waits for the gate  *)
(* while holding a tier. Holding every tier it needs, a handle is checked  *)
(* against the admission rules over the store's oldest and current schemas *)
(* and an upgrade in progress, and is attached or refused. A reader that   *)
(* cannot be left attached through an unsafe upgrade is kept out by the    *)
(* exclusive reader tier; a writer by the exclusive writer tier. Who holds *)
(* what follows from where each process is (att), so a death releases all  *)
(* of it. An upgrade moves the layer to its target in one step when it     *)
(* ends, and an upgrader that dies in its body leaves the upgrade recorded *)
(* in progress; the body is modelled further below (UpSteps). Escapes is   *)
(* whether a waiting process may give up or be refused; with it false and  *)
(* nothing dying, the only state without a successor in a deadlock-free    *)
(* model is every process ended.                                           *)
(*                                                                         *)
(* Clones and leases. A handle's clones share its attachment, and every    *)
(* operation on any of them holds a lease on it until the operation ends.  *)
(* A clone's operation is abstract -- it holds a lease and does nothing    *)
(* else -- and the process's own reads and writes are operations too. An   *)
(* upgrader first drains: new operations on every clone are refused and it *)
(* waits for the leases to end while it still holds its tiers, and only    *)
(* then releases them and converts. Called from inside an operation it     *)
(* would wait for that operation, which cannot end while the call is       *)
(* blocked, so the call is refused. A drain can be cancelled, which leaves *)
(* the handle attached; a conversion cancelled or failed after the drain   *)
(* leaves it detached, holding nothing and refusing every operation, and   *)
(* it may attach again as an ordinary admission or be dropped. A death     *)
(* ends every lease with its process.                                      *)
(*                                                                         *)
(* The registry (NSlots entries; none when it is 0). An entry has a claim  *)
(* lock and a liveness lock, both OS locks that a death releases. A handle *)
(* registers after its check and before its attach returns: it claims an   *)
(* entry without waiting, tries the liveness lock without waiting, which   *)
(* succeeds only if no handle holds the entry, writes its token, and lets  *)
(* go of the claim lock last, keeping the liveness lock for as long as it  *)
(* stays attached. If every entry is held it attaches without one and the  *)
(* registry records that. A handle unregisters before it lets go of its    *)
(* tiers, whether it is dropped or its upgrade converts it, and updates    *)
(* its entry under the claim lock when it starts or stops draining. Every  *)
(* change to an entry and every inspection of one is made under its claim  *)
(* lock, which is only tried. A listing runs on a thread of a process that *)
(* need not be attached, visits the entries in turn, reports one live only *)
(* if the liveness lock was held when it probed, and says whether it may   *)
(* be incomplete. A host crash discards the registry.                      *)
(*                                                                         *)
(* Transactions (Txn). A write is a transaction over a commit log: begin   *)
(* appends a record, ingest stages the bodies, prepare appends a record    *)
(* carrying the swap set, which is the commit point, the swaps move the    *)
(* namespace cells in turn, and end appends a record, after which the      *)
(* staging is swept. Nothing is visible in the namespace before prepare,   *)
(* so a failure before it is rolled back, with the staging swept and the   *)
(* log reset. A failure at or after prepare cannot be, since its outcome   *)
(* may be the commit: it sets the fence and leaves the count odd and the   *)
(* log unresolved, for recovery to finish from the log. The log is reset   *)
(* by a holder with nothing in flight, and never while it holds unresolved *)
(* work. With Txn false a write is the one abstract operation above. With  *)
(* MixedWrites each write chooses: it begins a transaction or makes its    *)
(* first effect directly, and while it is between its effects the log      *)
(* shows which, since only a transaction's leaves its prepare record there. *)
(*                                                                         *)
(* Recovery and discovery. A holder told that recovery is required, or     *)
(* whose own transaction failed, may run recovery, which has a capability  *)
(* ordinary writes do not: the fence refuses writes, not recovery.         *)
(* Recovery reads the commit log and nothing else. It makes the swaps a    *)
(* prepared transaction names, appends its end record and sweeps the       *)
(* staging; or, for one begun and not prepared, sweeps the staging and     *)
(* resets the log. It then publishes a count that is even and trusted, and *)
(* only then clears the fence. Every step is idempotent and takes what it  *)
(* does from durable state, so a recoverer that stops between steps, or    *)
(* dies, leaves the rest to the next, which finds the fence, the ownership *)
(* record or the untrusted state and runs again. Discovery is a call any   *)
(* attached handle can make: in one step it reads the fence, the restart   *)
(* state and the ownership record together with whether anyone holds       *)
(* exclusion, reports whether recovery is required, and changes nothing.   *)
(* The work of a live holder is not abandoned, so it is not reported.      *)
(*                                                                         *)
(* Recovery of an unlogged write (UnloggedRecovery). An unlogged write has *)
(* no log for recovery to read, and its half-applied store is left for the *)
(* STRANDED-VOTE guard to settle, which may roll it either way. With       *)
(* UnloggedRecovery a told holder may recover without Txn: it makes a      *)
(* store half applied whole, forward or back, and then ends as a           *)
(* transaction's recovery does. Without it and without Txn nothing         *)
(* recovers, and a fence once set stays set.                               *)
(*                                                                         *)
(* The upgrade body (UpSteps). An upgrader that holds its tiers takes      *)
(* writer exclusion as a writer does, and starts only from a store that    *)
(* needs no recovery: told that recovery is required it releases, and the  *)
(* upgrade is refused. Under exclusion, inside the count's bracket, it     *)
(* records the upgrade intent, makes UpSteps migration steps, each of them *)
(* durable, advances the layer's schema only after the last, and clears    *)
(* the intent. The body cannot be rolled back, so a failure in it, or the  *)
(* death of its upgrader, leaves the intent in place and recovery          *)
(* required, and recovery is resuming the upgrade from the step that       *)
(* durable state shows, by a handle that speaks the target schema. With    *)
(* UpSteps 0 an upgrade completes in one step, as it did before the body   *)
(* was modelled.                                                           *)
(*                                                                         *)
(* Cumulative reader compatibility. An upgrade of a layer has a floor, the *)
(* oldest reader schema that can read what it leaves. It is safe for       *)
(* readers when the oldest schema the store still admits can read that, so *)
(* it is judged against the oldest admissible reader schema and not the    *)
(* current one, and every schema above the oldest can read it too. Any     *)
(* other upgrade takes the reader tier exclusively, which drains every     *)
(* reader before the upgrade crosses that boundary, and sets the oldest    *)
(* schema to its target when it completes. An upgrade of one layer changes *)
(* nothing in another. The data floor of a layer is the oldest reader      *)
(* schema that can read what has been written to it; it never exceeds the  *)
(* oldest admissible schema, and no attached handle speaks a schema below  *)
(* it.                                                                     *)
(*                                                                         *)
(* Callbacks (Callbacks). The store calls back into a caller once an       *)
(* operation's effects are done, still inside it and under exclusion, as   *)
(* the commit observer is called. A callback waits on nothing, does not    *)
(* call into the store and returns, so it is one step of its holder,       *)
(* enabled whenever it is reached, that changes nothing but where its      *)
(* holder is (DESIGN-NOTES "No caller code runs under exclusion but the    *)
(* commit observer"). How long it takes is not the model's concern: no     *)
(* step has a duration.                                                    *)
(*                                                                         *)
(* Notifications (Notifications). Under it a wait does not see the change  *)
(* it waits for: the waiter samples, and if what it waits for is not there *)
(* it registers and sleeps until a notification wakes it, as a step of its *)
(* own, and then samples again and decides from what it reads              *)
(* (DESIGN-NOTES "A change to the coordination state is delivered as a     *)
(* notification"). A notification is a hint: a spurious one costs a        *)
(* sample, and a lost one leaves its waiter asleep.                        *)
(*                                                                         *)
(* Steps are sequentially consistent. The contract's ordering rules -- a   *)
(* raise is ordered with the holder's filesystem operations, and a sample  *)
(* with the sampler's, so a sample taken after observing a modification's  *)
(* effect reflects its opening raise -- hold here by construction. They    *)
(* are requirements on an implementation that this model assumes.          *)
(*                                                                         *)
(* Control selects a fault to confirm the properties detect it:            *)
(*   "none"                     the contract.                              *)
(*   "replace-without-fence"    an acquirer that finds an abandoned record *)
(*                              replaces it without setting the fence.     *)
(*   "unlock-before-clear"      a clean release drops the lock, then       *)
(*                              clears the record.                         *)
(*   "ignore-restart"           an acquirer does not consult whether the   *)
(*                              coordination state survived a restart.     *)
(*   "assembled-sample"         a sample is two parts read in two steps.   *)
(*   "late-invalidation"        a cached sample is invalidated some time   *)
(*                              after the publication that outdates it.    *)
(*   "read-untrusted-as-stored" state that cannot be vouched for is read   *)
(*                              as it is stored.                           *)
(*   "cache-always-kept"        a handle keeps its cache whatever the      *)
(*                              generation it finds.                       *)
(*   "callback-changes-the-store"                                          *)
(*                              a callback writes to the store.            *)
(*   "callback-waits-for-exclusion"                                        *)
(*                              a callback waits until nobody holds        *)
(*                              exclusion, which its own holder does.      *)
(*   "callback-never-returns"   a callback does not return.                *)
(*   "notification-lost"        a registered waiter's notification may be  *)
(*                              lost, so that nothing wakes it.            *)
(*   "notification-spurious"    a registered waiter may be woken with      *)
(*                              nothing changed.                           *)
(*   "upgrader-keeps-tiers"     an upgrader keeps its shared tiers while   *)
(*                              it waits for the gate exclusively.         *)
(*   "upgrader-holds-exclusion" an upgrader begins while it holds writer   *)
(*                              exclusion.                                 *)
(*   "unsafe-writer-tier-first" an upgrade unsafe for readers takes the    *)
(*                              writer tier before the reader tier.        *)
(*   "gate-held-through-writer-tier"                                       *)
(*                              a handle keeps the gate while it waits for *)
(*                              the writer tier.                           *)
(*   "unsafe-upgrade-leaves-readers"                                       *)
(*                              an upgrade unsafe for readers does not     *)
(*                              take the reader tier exclusively.          *)
(*   "check-before-writer-tier" a handle is checked before it waits for    *)
(*                              the writer tier and not again.             *)
(*   "upgrade-from-inside-an-operation"                                    *)
(*                              an upgrade is started from inside an       *)
(*                              operation of its own handle.               *)
(*   "drain-ignores-leases"     a conversion begins without waiting for    *)
(*                              the leases to end.                         *)
(*   "clone-starts-while-draining"                                         *)
(*                              a clone starts an operation on a handle    *)
(*                              that is draining.                          *)
(*   "cancelled-drain-detaches" cancelling a drain detaches the handle.    *)
(*   "cancelled-conversion-stays-usable"                                   *)
(*                              a conversion cancelled after the drain     *)
(*                              leaves the handle usable and detached.     *)
(*   "list-without-claim"       a listing inspects an entry without        *)
(*                              claiming it.                               *)
(*   "register-releases-claim-early"                                       *)
(*                              a handle lets go of the claim lock before  *)
(*                              it has written its token.                  *)
(*   "entry-follows-the-handles-life"                                      *)
(*                              an upgrader keeps its entry through the    *)
(*                              conversion that detaches it.               *)
(*   "register-without-claim"   a handle writes an entry without claiming  *)
(*                              it.                                        *)
(*   "reuse-without-liveness-probe"                                        *)
(*                              a handle takes an entry without trying its *)
(*                              liveness lock.                             *)
(*   "listing-infers-from-fullness"                                        *)
(*                              a listing says it may be incomplete only   *)
(*                              when it found no free entry.               *)
(*   "reset-during-a-transaction"                                          *)
(*                              the commit log is reset while a            *)
(*                              transaction is in flight.                  *)
(*   "reset-while-recovery-required"                                       *)
(*                              an acquirer told recovery is required      *)
(*                              resets the commit log.                     *)
(*   "apply-before-prepare"     the namespace is changed before the        *)
(*                              prepare record is written.                 *)
(*   "prepare-before-ingest"    the prepare record is written before       *)
(*                              ingest has staged the bodies.              *)
(*   "end-before-the-swaps"     the end record is written before every     *)
(*                              swap has landed.                           *)
(*   "failure-after-prepare-is-rolled-back"                                *)
(*                              a failure after prepare is handled as one  *)
(*                              before it: the staging is swept and the    *)
(*                              log reset, with the count made even.       *)
(*   "discovery-trusts-the-ownership-record"                               *)
(*                              discovery reports recovery required        *)
(*                              whenever an ownership record is there.     *)
(*   "discovery-ignores-the-fence"                                         *)
(*                              discovery does not read the fence.         *)
(*   "discovery-ignores-the-restart"                                       *)
(*                              discovery does not read the restart        *)
(*                              state.                                     *)
(*   "discovery-clears-the-fence"                                          *)
(*                              discovery clears a fence it finds set      *)
(*                              while nobody holds exclusion.              *)
(*   "fence-cleared-at-recovery-start"                                     *)
(*                              recovery clears the fence when it starts   *)
(*                              and not when it has finished.              *)
(*   "count-published-before-the-namespace-is-whole"                       *)
(*                              recovery publishes the trusted count       *)
(*                              before it has made the swaps.              *)
(*   "recovery-rolls-back-a-prepared-transaction"                          *)
(*                              recovery discards a prepared transaction   *)
(*                              instead of rolling it forward.             *)
(*   "step-before-the-intent"   a migration step is made before the intent *)
(*                              is recorded.                               *)
(*   "version-advanced-early"   the layer's schema advances before the     *)
(*                              last step.                                 *)
(*   "interrupted-upgrade-rolled-back"                                     *)
(*                              recovery of an interrupted upgrade clears  *)
(*                              the intent and its progress without        *)
(*                              advancing the schema.                      *)
(*   "interrupted-upgrade-admits-source-writers"                           *)
(*                              a writer that speaks only the source       *)
(*                              schema is admitted while an upgrade is     *)
(*                              interrupted.                               *)
(*   "upgrade-ignores-recovery-required"                                   *)
(*                              an upgrader told that recovery is required *)
(*                              goes on with the upgrade.                  *)
(*   "clone-starts-during-the-body"                                        *)
(*                              a clone starts an operation on a handle    *)
(*                              whose upgrade is in its body.              *)
(*   "unsafe-judged-against-the-current-schema"                            *)
(*                              an upgrade is judged safe for readers      *)
(*                              against the current schema and not the     *)
(*                              oldest admissible one.                     *)
(*   "unsafe-upgrade-raises-every-layer"                                   *)
(*                              an upgrade unsafe for readers raises the   *)
(*                              oldest reader schema of every layer and    *)
(*                              not only its own.                          *)
(*   "upgrade-rules-apply-to-every-layer"                                  *)
(*                              an upgrade of one layer changes what every *)
(*                              layer admits and not only its own.         *)
(*                                                                         *)
(* The Witness_ properties are false on purpose: a run that checks one     *)
(* must report it violated, which shows the state or step it names is      *)
(* reachable and so that the properties are not holding for want of        *)
(* anything to check.                                                      *)
(***************************************************************************)
EXTENDS Naturals, Integers, FiniteSets, Sequences, TLC

CONSTANTS
    Procs,        \* the processes that open the root, 1..Len(RoleOf)
    MaxRequests,  \* bound on exclusion requests, to keep the model finite
    MaxDeaths,    \* bound on process deaths
    MaxCrashes,   \* bound on host crashes
    MaxReads,     \* bound on reads
    MaxLeases,    \* bound on the operations one handle's clones have in flight
    NSlots,       \* how many entries the registry has; 0 leaves it out
    MaxListings,  \* bound on registry listings
    MaxDiscoveries, \* bound on recovery discovery calls
    MaxUpgrades,  \* bound on upgrade starts when the body is modelled (UpSteps > 0)
    Escapes,      \* whether a waiter may give up or be refused; FALSE makes a wait
                  \* that never ends a deadlock instead of a request that ends
    Txn,          \* whether a write is a transaction over a commit log and not
                  \* one abstract operation
    UnloggedRecovery, \* whether a told holder recovers an unlogged write
    MixedWrites,  \* whether each write chooses to be a transaction or not
    UpSteps,      \* how many migration steps an upgrade's body makes; 0 leaves the
                  \* body out, and an upgrade completes in one step
    Callbacks,    \* whether the store calls back into a caller under exclusion
    Notifications, \* whether a waiter sleeps until a notification wakes it
    Scenario,     \* which of the Scenarios the processes are
    Control       \* the fault under test

None == 0   \* a number, since TLC compares only values of one kind
NoVal == -1

\* A scenario fixes what each process is -- a reader, a writer, or an upgrader of one
\* layer -- and the schema each speaks in every layer, with the oldest and current
\* schemas each layer starts with. An upgrader moves its layer from the schema it
\* speaks to the next, and its upgrade has a floor: the oldest reader schema that can
\* read what the upgrade leaves. A configuration names a scenario because a
\* configuration file cannot state a sequence. The layers of a scenario start at
\* different schemas, so that a step that confuses one layer with another changes
\* something a property can see.
ScenL(roles, speaks, o, c, upl, upf) ==
    [roles   |-> roles,
     speaks  |-> speaks,
     oldest  |-> o,
     current |-> c,
     upl     |-> upl,
     upf     |-> upf]

\* One layer, whose upgrader is safe for readers when every reader schema that is
\* admissible can read what it leaves, and unsafe when only the schema it moves to can.
Scen(roles, speaks, o, c, t) ==
    ScenL([p \in 1..Len(roles) |->
              IF roles[p] \in {"safeUpgrader", "unsafeUpgrader"} THEN "upgrader"
                                                                 ELSE roles[p]],
          [p \in 1..Len(roles) |-> <<speaks[p]>>], <<o>>, <<c>>,
          [p \in 1..Len(roles) |-> 1],
          [p \in 1..Len(roles) |-> IF roles[p] = "unsafeUpgrader" THEN t ELSE o])

Scenarios ==
       "Plain2" :> Scen(<<"writer", "writer">>, <<1, 1>>, 1, 1, 1)
    @@ "Plain3" :> Scen(<<"writer", "writer", "writer">>, <<1, 1, 1>>, 1, 1, 1)
    @@ "ReaderAndWriter" :> Scen(<<"reader", "writer">>, <<1, 2>>, 1, 2, 3)
    @@ "ReaderAndWriterTooNew" :> Scen(<<"reader", "writer">>, <<3, 3>>, 1, 2, 3)
    @@ "SafeAndReader" :> Scen(<<"safeUpgrader", "reader">>, <<2, 1>>, 1, 2, 3)
    @@ "SafeAndTargetReader" :> Scen(<<"safeUpgrader", "reader">>, <<2, 3>>, 1, 2, 3)
    @@ "UnsafeAndReader" :> Scen(<<"unsafeUpgrader", "reader">>, <<2, 1>>, 1, 2, 3)
    @@ "SafeAndWriter" :> Scen(<<"safeUpgrader", "writer">>, <<2, 2>>, 1, 2, 3)
    @@ "SafeAndTargetWriter" :> Scen(<<"safeUpgrader", "writer">>, <<2, 3>>, 1, 2, 3)
    @@ "UnsafeAndWriter" :> Scen(<<"unsafeUpgrader", "writer">>, <<2, 2>>, 1, 2, 3)
    @@ "TwoSafe" :> Scen(<<"safeUpgrader", "safeUpgrader">>, <<2, 2>>, 1, 2, 3)
    @@ "UnsafeAndTwoReaders" :>
          Scen(<<"unsafeUpgrader", "reader", "reader">>, <<2, 1, 1>>, 1, 2, 3)
    @@ "SafeWriterReader" :>
          Scen(<<"safeUpgrader", "writer", "reader">>, <<2, 2, 1>>, 1, 2, 3)
    @@ "UnsafeWriterReader" :>
          Scen(<<"unsafeUpgrader", "writer", "reader">>, <<2, 2, 1>>, 1, 2, 3)
    @@ "SafeUnsafeWriter" :>
          Scen(<<"safeUpgrader", "unsafeUpgrader", "writer">>, <<2, 2, 2>>, 1, 2, 3)
    @@ "TwoSafeWriter" :>
          Scen(<<"safeUpgrader", "safeUpgrader", "writer">>, <<2, 2, 2>>, 1, 2, 3)
    @@ "SafeTargetWriterReader" :>
          Scen(<<"safeUpgrader", "writer", "reader">>, <<2, 3, 1>>, 1, 2, 3)
    @@ "UnsafeTargetWriterReader" :>
          Scen(<<"unsafeUpgrader", "writer", "reader">>, <<2, 3, 1>>, 1, 2, 3)
    @@ "SafeTwoTargetWriters" :>
          Scen(<<"safeUpgrader", "writer", "writer">>, <<2, 3, 3>>, 1, 2, 3)
    @@ "ChainSafe" :>
          ScenL(<<"upgrader", "upgrader", "reader">>, <<<<1>>, <<2>>, <<1>>>>,
                <<1>>, <<1>>, <<1, 1, 1>>, <<1, 1, 1>>)
    @@ "ChainFloor" :>
          ScenL(<<"upgrader", "upgrader", "reader">>, <<<<1>>, <<2>>, <<1>>>>,
                <<1>>, <<1>>, <<1, 1, 1>>, <<1, 2, 1>>)
    @@ "TwoLayersSafe" :>
          ScenL(<<"upgrader", "writer", "reader">>, <<<<1, 3>>, <<2, 3>>, <<1, 1>>>>,
                <<1, 1>>, <<1, 3>>, <<1, 1, 1>>, <<1, 1, 1>>)
    @@ "TwoLayersUnsafe" :>
          ScenL(<<"upgrader", "writer", "reader">>, <<<<1, 3>>, <<2, 3>>, <<1, 1>>>>,
                <<1, 1>>, <<1, 3>>, <<1, 1, 1>>, <<2, 1, 1>>)
    @@ "TwoLayerUpgradesSafe" :>
          ScenL(<<"upgrader", "upgrader", "reader">>,
                <<<<1, 3>>, <<2, 3>>, <<1, 1>>>>,
                <<1, 1>>, <<1, 3>>, <<1, 2, 1>>, <<1, 1, 1>>)
    @@ "TwoLayerUpgradesUnsafe" :>
          ScenL(<<"upgrader", "upgrader", "reader">>,
                <<<<1, 3>>, <<2, 3>>, <<1, 1>>>>,
                <<1, 1>>, <<1, 3>>, <<1, 2, 1>>, <<1, 4, 1>>)
    @@ "TwoLayerUpgradesFloor" :>
          ScenL(<<"upgrader", "upgrader", "reader">>,
                <<<<1, 3>>, <<2, 3>>, <<1, 1>>>>,
                <<1, 1>>, <<1, 3>>, <<1, 2, 1>>, <<1, 3, 1>>)

RoleOf == Scenarios[Scenario].roles
SpeaksOf == Scenarios[Scenario].speaks
Oldest == Scenarios[Scenario].oldest
Current == Scenarios[Scenario].current
\* The layer an upgrader upgrades, and the floor of its upgrade.
UpL(p) == Scenarios[Scenario].upl[p]
UpF(p) == Scenarios[Scenario].upf[p]
\* The schema an upgrade moves its layer to: the next after the one its upgrader speaks.
UpTarget(p) == SpeaksOf[p][UpL(p)] + 1

Max(a, b) == IF a >= b THEN a ELSE b

\* The next and previous value of the count, the generation or a version, and whether a
\* bounded step has room left and its count once it is taken.
Nx(x) == x + 1
Pv(x) == x - 1
Room(c, m) == c < m
Tick(c) == c + 1

ASSUME Scenario \in DOMAIN Scenarios
ASSUME Procs # {} /\ Procs = 1..Len(RoleOf)
ASSUME MaxRequests \in Nat /\ MaxDeaths \in Nat /\ MaxCrashes \in Nat
ASSUME MaxReads \in Nat /\ MaxLeases \in Nat /\ Escapes \in BOOLEAN
ASSUME Txn \in BOOLEAN /\ UnloggedRecovery \in BOOLEAN
ASSUME UnloggedRecovery => ~Txn
ASSUME MixedWrites \in BOOLEAN /\ (MixedWrites => UnloggedRecovery)
ASSUME UpSteps \in Nat /\ (UpSteps > 0 => Txn)
ASSUME UpSteps > 0 => Control # "upgrader-holds-exclusion"
ASSUME NSlots \in Nat /\ MaxListings \in Nat /\ MaxDiscoveries \in Nat
ASSUME MaxUpgrades \in Nat /\ Callbacks \in BOOLEAN /\ Notifications \in BOOLEAN
ASSUME \A p \in Procs : RoleOf[p] \in {"reader", "writer", "upgrader"}
ASSUME \A l \in 1..Len(Oldest) : Oldest[l] <= Current[l]
ASSUME Control \in {"none", "replace-without-fence", "unlock-before-clear",
                    "ignore-restart", "assembled-sample", "late-invalidation",
                    "read-untrusted-as-stored", "cache-always-kept",
                    "callback-changes-the-store", "callback-waits-for-exclusion",
                    "callback-never-returns", "notification-lost",
                    "notification-spurious", "upgrader-keeps-tiers",
                    "upgrader-holds-exclusion", "unsafe-writer-tier-first",
                    "gate-held-through-writer-tier",
                    "unsafe-upgrade-leaves-readers", "check-before-writer-tier",
                    "upgrade-from-inside-an-operation", "drain-ignores-leases",
                    "clone-starts-while-draining", "cancelled-drain-detaches",
                    "cancelled-conversion-stays-usable",
                    "list-without-claim", "register-releases-claim-early",
                    "entry-follows-the-handles-life", "register-without-claim",
                    "reuse-without-liveness-probe",
                    "listing-infers-from-fullness",
                    "reset-during-a-transaction", "reset-while-recovery-required",
                    "apply-before-prepare", "prepare-before-ingest",
                    "end-before-the-swaps", "failure-after-prepare-is-rolled-back",
                    "discovery-trusts-the-ownership-record",
                    "discovery-ignores-the-fence", "discovery-ignores-the-restart",
                    "discovery-clears-the-fence",
                    "fence-cleared-at-recovery-start",
                    "count-published-before-the-namespace-is-whole",
                    "recovery-rolls-back-a-prepared-transaction",
                    "step-before-the-intent", "version-advanced-early",
                    "interrupted-upgrade-rolled-back",
                    "interrupted-upgrade-admits-source-writers",
                    "upgrade-ignores-recovery-required",
                    "clone-starts-during-the-body",
                    "unsafe-judged-against-the-current-schema",
                    "unsafe-upgrade-raises-every-layer",
                    "upgrade-rules-apply-to-every-layer"}

Layers == 1..Len(Oldest)

\* The most acquisitions of exclusion: one for each request, and one for each upgrade
\* that takes it for its body.
Acquisitions == MaxRequests + MaxUpgrades

\* The most any count or sample can reach: at most three raises for each acquisition
\* -- the opening, the closing and a recovery's -- and one for a sample of state
\* that cannot be vouched for.
MaxCount == 3 * Acquisitions + 1

\* The most the version of the shared durable state can reach: each transaction
\* changes it at most once for each record it appends or resets.
MaxShared == 6 * Acquisitions

VARIABLES
    pc,          \* [Procs -> process states]: each process's execution state
    req,         \* [Procs -> request states]: the state of its latest request
    eligible,    \* [Procs -> BOOLEAN]: whether the admission rules admit it
    lockHolder,  \* the process holding the OS lock, or None
    owner,       \* the process the ownership record names, or None
    fence,       \* recovery is required until it is cleared; survives a death
    trusted,     \* the coordination state is known to have been maintained
    count,       \* the published modification count
    gen,         \* the generation, advanced on every acquisition of exclusion
    a, b,        \* the two cells of a write; a # b is a store half applied
    shared,      \* durable state a holder may cache
    log,         \* the commit log's tail: "empty", or the latest transaction's
                 \* last record: "begun", "prepared" or "ended"
    stage,       \* the version whose bodies are staged, or 0
    ptgt,        \* the version the prepare record's swap set moves to, or 0
    mygen,       \* [Procs -> gen or NoVal]: what each handle last advanced to
    cache,       \* [Procs -> shared or NoVal]: each handle's knowledge of it
    rphase,      \* [Procs -> reader phase]: where each reader is in its read
    rbefore,     \* [Procs -> sample or NoVal]: a reader's first sample
    rseenA,      \* [Procs -> a or NoVal]: what a reader observed of a
    rseenB,      \* [Procs -> b or NoVal]: what a reader observed of b
    rpart,       \* [Procs -> part or NoVal]: half an assembled sample
    scache,      \* [Procs -> sample or NoVal]: each reader's cached sample
    disc,        \* [Procs -> report]: what each handle's last discovery said
    att,         \* [Procs -> attachment states]: where each is in attaching or
                 \* converting, which fixes the gate and tiers it holds
    areq,        \* [Procs -> request states]: the state of its latest attach or
                 \* upgrade request
    lease,       \* [Procs -> 0..MaxLeases]: operations its clones have in flight
    cl,          \* [Slots -> Procs or None]: who holds each entry's claim lock
    ll,          \* [Slots -> Procs or None]: who holds each entry's liveness lock
    etok,        \* [Slots -> Procs or None]: the token written in each entry
    edr,         \* [Slots -> BOOLEAN]: whether the entry says its handle drains
    ovf,         \* some handle attached unregistered because no entry was free
    rst,         \* [Procs -> registry state]: where each is in registering,
                 \* updating or unregistering its entry
    rscan,       \* [Procs -> SUBSET Slots]: the entries a registrant has not tried
    rcur,        \* [Procs -> Slots or None]: the entry a registrant works on
    lph,         \* [Procs -> listing phase]: where each one's listing thread is
    lcur,        \* [Procs -> 1..NSlots+1]: the next entry its listing visits
    ltok,        \* [Procs -> token or None]: the token its listing has read
    lrep,        \* [Procs -> report]: the entry its listing last reported on
    lunk,        \* [Procs -> BOOLEAN]: its listing met an entry it could not claim
    lfree,       \* [Procs -> BOOLEAN]: its listing met an entry that was not live
    lres,        \* [Procs -> listing result]: what its last listing said
    lists,       \* how many listings have been started
    oldest,      \* [Layers -> schema]: the oldest admissible reader schema
    current,     \* [Layers -> schema]: the current schema
    upState,     \* "ordinary", or an upgrade in progress, "safe" or "unsafe" for
                 \* readers; it survives its upgrader's death
    ustep,       \* how many migration steps of the recorded upgrade are durable
    ulayer,      \* 0, or the layer the recorded upgrade moves
    utgt,        \* the schema the recorded upgrade moves that layer to
    ufloor,      \* the oldest reader schema that can read what it leaves
    dfloor,      \* [Layers -> schema]: the oldest reader schema that can read the
                 \* data of each layer as it is
    rtier,       \* [Procs -> BOOLEAN]: whether an upgrader holds the reader tier
                 \* exclusively, decided when it takes its first tier
    diedIn,      \* ghost: where the latest holder died with the record present
    requests,
    deaths,
    crashes,
    reads,
    discoveries,
    upgrades,
    woken        \* [Procs -> Notices]: whether each process samples what it waits
                 \* for, sleeps registered for a notification, or has lost it

vars == <<pc, req, eligible, lockHolder, owner, fence, trusted, count, gen,
          a, b, shared, log, stage, ptgt, mygen, cache, rphase, rbefore, rseenA,
          rseenB, rpart, scache, disc, att, areq, lease, cl, ll, etok, edr, ovf,
          rst, rscan, rcur, lph, lcur, ltok, lrep, lunk, lfree, lres, lists,
          oldest, current, upState, ustep, ulayer, utgt, ufloor, dfloor, rtier,
          diedIn, requests, deaths, crashes, reads,
          discoveries, upgrades, woken>>

coord  == <<lockHolder, owner, fence, trusted, count, gen>>
store  == <<a, b, shared, log, stage, ptgt>>
handle == <<mygen, cache>>
reader == <<rphase, rbefore, rseenA, rseenB, rpart, scache, disc>>
\* The bounded steps' counters, and each process's notice of a change, which only the
\* notification's own steps change, so that every step that names counts leaves it alone.
counts == <<requests, deaths, crashes, reads, discoveries, upgrades, woken>>
\* What the store records about schemas and the upgrade in progress.
recorded == <<oldest, current, dfloor, upState, ulayer, utgt, ufloor, ustep>>
schema == <<recorded, rtier>>
\* The registry: the entries' locks and data, each handle's place in keeping its
\* entry, and each listing's.
rlock  == <<cl, ll>>
rdata  == <<etok, edr>>
rown   == <<rst, rscan, rcur>>
rlist  == <<lph, lcur, ltok, lrep, lunk, lfree, lres, lists>>
reg    == <<rlock, rdata, ovf, rown, rlist>>
adm    == <<att, areq, lease, reg, schema>>
\* Everything but the process state, the attachment state, its request, the
\* leases, the registry and the schema state.
bystanders == <<req, eligible, coord, store, handle, reader, diedIn, counts>>
rest   == <<lease, reg, bystanders>>
\* Everything but the registry.
core   == <<pc, att, areq, lease, schema, bystanders>>

\* Process states. "new" has not started; "attaching" is passing the gate and
\* taking its tiers, and "converting" is an upgrader doing the same exclusively;
\* "convertingX" is one doing it while holding writer exclusion, only under
\* Control = "upgrader-holds-exclusion". "locked" holds the lock and has not yet
\* replaced the record.
\* A holder is then "held" before an operation; "opening" while the opening
\* raise is prepared and not yet published; "open" once it is published and
\* before the first effect; "writing" between the effects; "wrote" after the last;
\* "observed" once a callback has returned, under Callbacks;
\* "closing" while the closing raise is prepared and not yet published; and
\* "closed" once the operation is over. Under Txn an operation goes from "open"
\* through "begun" (the begin record is in the log), "ingested" (the bodies are
\* staged), "prepared" (the prepare record is in the log: the commit point),
\* "writing" (between the swaps), "applied" (every swap has
\* landed) and "ended" (the end record is in the log) to "wrote". "told" holds
\* exclusion having been told recovery is required, or having failed
\* unresolved, and may only release it or run recovery. A recoverer is in
\* "recSwap1" and "recSwap2" while it makes the swaps a prepared transaction
\* names, "recEnd" while it appends that transaction's end record or sweeps one
\* that was begun, "recReset" while it resets the log of one that was, "recSweep"
\* while it sweeps the staging, "recWhole" while it makes an unlogged write's
\* half-applied store whole, "recCount" while it publishes the even count
\* together with the trusted state, and "recClear" while it clears the fence.
\* Under UpSteps an upgrader holds exclusion for its body and goes through
\* "locked", "held", "opening" and "open" as a writer does; "upBody" is it, or a
\* writer's operation, between recording the upgrade intent and clearing it, and
\* "recUp" a recoverer doing the same for an upgrade it resumes. "releasing" has
\* cleared the record and still holds the lock;
\* "unlocked" is only reached under Control = "unlock-before-clear", and has
\* dropped the lock but not cleared the record. "detached" is a handle whose
\* conversion was cancelled or failed: it holds nothing, refuses every
\* operation, and may attach again or be dropped. "detaching" is a handle that
\* is being dropped and is leaving the registry before it lets go of its tiers.
LiveStates == {"attaching", "converting", "convertingX", "idle", "waiting",
               "locked", "held", "opening", "open", "begun", "ingested",
               "prepared", "writing", "applied", "ended", "wrote", "observed",
               "closing", "closed", "told", "releasing", "unlocked", "detached",
               "detaching", "recSwap1", "recSwap2", "recEnd", "recReset",
               "recSweep", "recCount", "recClear", "upBody", "recUp",
               "recWhole"}
\* Where a process is inside an operation of its own: a write from its request
\* to its release. A read is inside one while rphase is not "none".
InOperationStates == LiveStates \ {"attaching", "converting", "convertingX",
                                   "idle", "detached", "detaching"}
States == {"new", "exited", "dead"} \cup LiveStates
OpenStates == {"open", "begun", "ingested", "prepared", "writing",
               "applied", "ended", "wrote", "observed", "closing", "upBody"}
\* Where a holder is inside a transaction, between its begin record and the
\* sweep of its staging.
TxnStates == {"begun", "ingested", "prepared", "writing", "applied",
              "ended"}
\* Where a holder is running recovery, which holds exclusion and the record.
RecStates == {"recWhole", "recSwap1", "recSwap2", "recEnd", "recReset", "recSweep",
              "recCount", "recClear", "recUp"}
DiscReports == {"none", "required", "notRequired"}
OwnsRecord == OpenStates \cup {"held", "opening", "closed", "told", "convertingX"}
                         \cup RecStates
HoldsLock == OwnsRecord \cup {"locked", "releasing"}
Outcomes == {"served", "withdrawn", "dead", "refused"}
DiedKinds == {"none", "crash", "unlocked"} \cup OwnsRecord
ReaderPhases == {"none", "before2", "obsA", "obsB", "after1", "after2"}
\* A process "sample"s what it waits for before it waits, and again once woken; "asleep" is
\* registered for a notification; "lost" was, and its notification will never come.
Notices == {"sample", "asleep", "lost"}

\* Attachment states. An ordinary attach goes wGate (asks for the gate shared),
\* hasGate (holds it, asks for the reader tier), gateRd (holds both, about to
\* drop the gate), rd (holds the reader tier, asks for the writer tier), tiers
\* (holds every tier it needs, check pending), registering (checked and
\* holding its tiers, writing its entry in the registry, which is skipped when
\* there is none), then attached, or refused and gone. A handle leaves its
\* attachment through unregistering, which clears its entry while it still holds
\* its tiers. "gateWr" and "rdChecked" are only reached under a control. An
\* upgrade goes draining (still attached and holding its tiers, refusing new
\* operations and waiting for the leases to end), uWGate (holding nothing,
\* asks for the gate exclusively; uWGateT holds its shared tiers, only under a
\* control), uGate (holds the gate), uRd or uWr (also one tier), uTiers (every
\* tier it needs), then uBody (the upgrade is in progress).
AttStates == {"off", "wGate", "hasGate", "gateRd", "gateWr", "rd", "rdChecked",
              "tiers", "registering", "attached", "unregistering", "draining",
              "uWGate", "uWGateT", "uGate", "uRd", "uWr", "uTiers", "uBody"}

\* The registry's entries, and what a registrant, an owner and a listing can be
\* doing. A registrant is "scan"ning for an entry, then has "claimed" one,
\* holds its "locked" liveness lock, has "written" its token, and is "in" once it
\* has let go of the claim lock; "unreg" is a handle that found every entry held
\* and attached without one. An owner "upd"ates its entry under its claim lock,
\* and unregisters by claiming ("uc"), clearing ("ucl") and releasing.
Slots == 1..NSlots
RegistryOn == NSlots > 0
RegStates == {"out", "scan", "claimed", "locked", "written", "in", "upd", "uc",
              "ucl", "unreg"}
ListPhases == {"none", "next", "held", "read"}
ListStatuses == {"free", "dead", "live", "unknown"}
NoRep == <<0, 0, "none">>
HoldsAttachment == {"registering", "attached", "unregistering", "draining"}

Set(f, k, v) == [f EXCEPT ![k] = v]
Odd(n) == n % 2 = 1
Even(n) == n % 2 = 0

CanWrite(p) == RoleOf[p] # "reader"
Upgrader(p) == RoleOf[p] = "upgrader"

\* An upgrade counts as safe for readers only if it is safe for every reader schema
\* that is still admissible, from the oldest up: what it leaves can be read by the
\* oldest. Under Control = "unsafe-judged-against-the-current-schema" it is judged
\* against the current schema alone.
SafeFor(p) ==
    UpF(p) <= (IF Control = "unsafe-judged-against-the-current-schema"
                  THEN current[UpL(p)] ELSE oldest[UpL(p)])

\* Whether an upgrade takes the reader tier exclusively, which drains every
\* reader, is decided when its upgrader takes its first tier: an upgrade that is
\* not safe for readers does, unless a control says otherwise. The schemas cannot
\* change after that until the upgrade completes, since it holds the gate.
DecideReaderTier(p) == ~SafeFor(p) /\ Control # "unsafe-upgrade-leaves-readers"
TakesReaderTier(p) == rtier[p]

\* The gate and the tiers each process holds, fixed by where it is.
HoldsGateS(p) == att[p] \in {"hasGate", "gateRd", "gateWr"}
HoldsGateX(p) ==
    \/ att[p] \in {"uGate", "uRd", "uWr", "uTiers"}
    \/ att[p] = "uBody" /\ TakesReaderTier(p)
HoldsReaderS(p) ==
    \/ att[p] \in {"gateRd", "gateWr", "rd", "rdChecked", "tiers", "registering",
                   "attached", "unregistering", "draining"}
    \/ att[p] = "uWGateT"
HoldsWriterS(p) ==
    /\ CanWrite(p)
    /\ att[p] \in {"tiers", "registering", "attached", "unregistering",
                   "draining", "uWGateT"}
HoldsReaderX(p) ==
    \/ att[p] = "uRd"
    \/ att[p] \in {"uTiers", "uBody"} /\ TakesReaderTier(p)
HoldsWriterX(p) == att[p] \in {"uWr", "uTiers", "uBody"}

\* The operations in flight on a handle: its clones' and its own read. A write
\* of its own is inside one while pc says so.
Leases(p) == lease[p] + (IF rphase[p] # "none" THEN 1 ELSE 0)
InsideOperation(p) == rphase[p] # "none" \/ pc[p] \in InOperationStates

GateSHeld == \E q \in Procs : HoldsGateS(q)
GateXHeld == \E q \in Procs : HoldsGateX(q)
ReaderSHeld == \E q \in Procs : HoldsReaderS(q)
ReaderXHeld == \E q \in Procs : HoldsReaderX(q)
WriterSHeld == \E q \in Procs : HoldsWriterS(q)
WriterXHeld == \E q \in Procs : HoldsWriterX(q)

\* Whether an upgrade of layer l is in progress, which changes what it admits. Under
\* Control = "upgrade-rules-apply-to-every-layer" an upgrade of any layer counts as
\* in progress for every layer.
InProgress(l) ==
    upState # "ordinary" /\ (l = ulayer \/ Control = "upgrade-rules-apply-to-every-layer")

\* The admission rules, per layer, over what the store records (DESIGN-NOTES
\* "Schemas and attachment"). An upgrade of one layer changes the rules of that
\* layer alone.
ReaderOK(l, s) ==
    IF InProgress(l)
       THEN upState = "safe"
            /\ ((oldest[l] <= s /\ s <= current[l]) \/ s = utgt)
       ELSE oldest[l] <= s /\ s <= current[l]
WriterOK(l, s) ==
    IF InProgress(l)
       THEN s = utgt
            \/ (Control = "interrupted-upgrade-admits-source-writers"
                /\ s = current[l])
       ELSE s = current[l]
Admissible(p) ==
    \A l \in Layers :
        IF CanWrite(p) THEN WriterOK(l, SpeaksOf[p][l])
                       ELSE ReaderOK(l, SpeaksOf[p][l])

\* A store with its write half applied.
Partial == a # b

\* A commit log whose latest transaction is neither ended nor rolled back.
UnresolvedLog == log \in {"begun", "prepared"}

\* An upgrade whose intent is recorded and not yet cleared, whether or not its
\* upgrader lives.
IntentStands == UpSteps > 0 /\ upState # "ordinary"

\* Whether the recorded upgrade has moved its layer to its target schema, and
\* whether p's own upgrade has, whether or not an intent still stands.
Advanced == ulayer # 0 /\ current[ulayer] = utgt
Completed(p) == current[UpL(p)] = UpTarget(p)

\* The oldest reader schemas once the recorded upgrade completes: an upgrade unsafe
\* for readers sets its own layer's to its target, whatever its floor, and any other
\* leaves them as they are. Under Control = "unsafe-upgrade-raises-every-layer" every
\* layer's is set to its current schema.
OldestAfterCompleting ==
    IF upState # "unsafe" THEN oldest
    ELSE IF Control = "unsafe-upgrade-raises-every-layer"
            THEN [l \in Layers |-> IF l = ulayer THEN utgt ELSE current[l]]
            ELSE Set(oldest, ulayer, utgt)

\* Everything recovery may have to finish.
UnresolvedWork == Partial \/ UnresolvedLog \/ stage # 0 \/ IntentStands

\* A store with nothing left for recovery to finish: the namespace whole, the log
\* resolved, no staging and no upgrade intent standing.
Settled == a = b /\ log \in {"empty", "ended"} /\ stage = 0 /\ ~IntentStands

\* What a sampler finds when it reads the published count. State that cannot be
\* vouched for reads as odd, whatever is stored.
Visible ==
    IF trusted \/ Control = "read-untrusted-as-stored"
       THEN count
       ELSE IF Odd(count) THEN count ELSE Nx(count)

\* The value a reader's next single-step sample returns: its cached sample if
\* it holds one, otherwise what it reads.
Sampled(p) == IF scache[p] # NoVal THEN scache[p] ELSE Visible

\* Store::undisturbed: two samples bracket a span in which nothing was modified.
Undisturbed(x, y) == x = y /\ Even(x)

-----------------------------------------------------------------------------
\* Audience: internal.
TypeOK ==
    /\ pc \in [Procs -> States]
    /\ req \in [Procs -> {"none", "pending"} \cup Outcomes]
    /\ eligible \in [Procs -> BOOLEAN]
    /\ lockHolder \in Procs \cup {None}
    /\ owner \in Procs \cup {None}
    /\ fence \in BOOLEAN
    /\ trusted \in BOOLEAN
    /\ count \in 0..MaxCount
    /\ gen \in 0..Acquisitions
    /\ a \in 0..MaxRequests
    /\ b \in 0..MaxRequests
    /\ shared \in 0..MaxShared
    /\ log \in {"empty", "begun", "prepared", "ended"}
    /\ stage \in 0..MaxRequests
    /\ ptgt \in 0..MaxRequests
    /\ mygen \in [Procs -> NoVal..Acquisitions]
    /\ cache \in [Procs -> NoVal..MaxShared]
    /\ rphase \in [Procs -> ReaderPhases]
    /\ rbefore \in [Procs -> NoVal..MaxCount]
    /\ rseenA \in [Procs -> NoVal..MaxRequests]
    /\ rseenB \in [Procs -> NoVal..MaxRequests]
    /\ rpart \in [Procs -> NoVal..MaxCount]
    /\ scache \in [Procs -> NoVal..MaxCount]
    /\ disc \in [Procs -> DiscReports]
    /\ att \in [Procs -> AttStates]
    /\ areq \in [Procs -> {"none", "pending"} \cup Outcomes]
    /\ lease \in [Procs -> 0..MaxLeases]
    /\ cl \in [Slots -> Procs \cup {None}]
    /\ ll \in [Slots -> Procs \cup {None}]
    /\ etok \in [Slots -> Procs \cup {None}]
    /\ edr \in [Slots -> BOOLEAN]
    /\ ovf \in BOOLEAN
    /\ rst \in [Procs -> RegStates]
    /\ rscan \in [Procs -> SUBSET Slots]
    /\ rcur \in [Procs -> 0..NSlots]
    /\ lph \in [Procs -> ListPhases]
    /\ lcur \in [Procs -> 1..(NSlots + 1)]
    /\ ltok \in [Procs -> Procs \cup {None}]
    /\ lrep \in [Procs -> {NoRep} \cup (Slots \X (Procs \cup {None}) \X ListStatuses)]
    /\ lunk \in [Procs -> BOOLEAN]
    /\ lfree \in [Procs -> BOOLEAN]
    /\ lres \in [Procs -> {"none", "complete", "incomplete"}]
    /\ lists \in 0..MaxListings
    /\ oldest \in [Layers -> Nat]
    /\ current \in [Layers -> Nat]
    /\ upState \in {"ordinary", "safe", "unsafe"}
    /\ ustep \in 0..UpSteps
    /\ ulayer \in 0..Len(Oldest)
    /\ utgt \in Nat
    /\ ufloor \in Nat
    /\ dfloor \in [Layers -> Nat]
    /\ rtier \in [Procs -> BOOLEAN]
    /\ diedIn \in DiedKinds
    /\ requests \in 0..MaxRequests
    /\ deaths \in 0..MaxDeaths
    /\ crashes \in 0..MaxCrashes
    /\ reads \in 0..MaxReads
    /\ discoveries \in 0..MaxDiscoveries
    /\ upgrades \in 0..MaxUpgrades
    /\ woken \in [Procs -> Notices]

Init ==
    /\ pc = [p \in Procs |-> "new"]
    /\ req = [p \in Procs |-> "none"]
    /\ eligible = [p \in Procs |-> TRUE]
    /\ lockHolder = None
    /\ owner = None
    /\ fence = FALSE
    /\ trusted = TRUE
    /\ count = 0
    /\ gen = 0
    /\ a = 0
    /\ b = 0
    /\ shared = 0
    /\ log = "empty"
    /\ stage = 0
    /\ ptgt = 0
    /\ mygen = [p \in Procs |-> NoVal]
    /\ cache = [p \in Procs |-> NoVal]
    /\ rphase = [p \in Procs |-> "none"]
    /\ rbefore = [p \in Procs |-> NoVal]
    /\ rseenA = [p \in Procs |-> NoVal]
    /\ rseenB = [p \in Procs |-> NoVal]
    /\ rpart = [p \in Procs |-> NoVal]
    /\ scache = [p \in Procs |-> NoVal]
    /\ disc = [p \in Procs |-> "none"]
    /\ att = [p \in Procs |-> "off"]
    /\ areq = [p \in Procs |-> "none"]
    /\ lease = [p \in Procs |-> 0]
    /\ cl = [s \in Slots |-> None]
    /\ ll = [s \in Slots |-> None]
    /\ etok = [s \in Slots |-> None]
    /\ edr = [s \in Slots |-> FALSE]
    /\ ovf = FALSE
    /\ rst = [p \in Procs |-> "out"]
    /\ rscan = [p \in Procs |-> {}]
    /\ rcur = [p \in Procs |-> 0]
    /\ lph = [p \in Procs |-> "none"]
    /\ lcur = [p \in Procs |-> 1]
    /\ ltok = [p \in Procs |-> None]
    /\ lrep = [p \in Procs |-> NoRep]
    /\ lunk = [p \in Procs |-> FALSE]
    /\ lfree = [p \in Procs |-> FALSE]
    /\ lres = [p \in Procs |-> "none"]
    /\ lists = 0
    /\ oldest = [l \in Layers |-> Oldest[l]]
    /\ current = [l \in Layers |-> Current[l]]
    /\ upState = "ordinary"
    /\ ustep = 0
    /\ ulayer = 0
    /\ utgt = 0
    /\ ufloor = 0
    /\ dfloor = [l \in Layers |-> Oldest[l]]
    /\ rtier = [p \in Procs |-> FALSE]
    /\ diedIn = "none"
    /\ requests = 0
    /\ deaths = 0
    /\ crashes = 0
    /\ reads = 0
    /\ discoveries = 0
    /\ upgrades = 0
    /\ woken = [p \in Procs |-> "sample"]

\* What a process forgets when it ends: its handle's memory and any read.
Forget(p) ==
    /\ mygen' = Set(mygen, p, NoVal)
    /\ cache' = Set(cache, p, NoVal)
    /\ rphase' = Set(rphase, p, "none")
    /\ rbefore' = Set(rbefore, p, NoVal)
    /\ rseenA' = Set(rseenA, p, NoVal)
    /\ rseenB' = Set(rseenB, p, NoVal)
    /\ rpart' = Set(rpart, p, NoVal)
    /\ scache' = Set(scache, p, NoVal)
    /\ disc' = Set(disc, p, "none")

=============================================================================
