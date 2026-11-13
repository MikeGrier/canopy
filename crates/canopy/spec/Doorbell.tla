\* Copyright (c) Michael Grier.
------------------------------ MODULE Doorbell ------------------------------
(***************************************************************************)
(* The mechanism of a doorbell (DESIGN-NOTES "The ring: a portable         *)
(* protocol over one platform primitive, behind a contract spec"), each    *)
(* step one atomic operation or fence point of the code that realizes it.  *)
(* A producer publishes its work, then swaps the record of being rung to   *)
(* set and raises the signal only if it was clear, so a ring while the     *)
(* doorbell is already rung skips its system call. The consumer takes      *)
(* everything published, lowers the signal, clears the record, checks      *)
(* again for work and, finding none, waits for the signal; finding some,   *)
(* it takes it and checks again without lowering or clearing, so a busy    *)
(* consumer pays no system call per batch. The signal is a level: a raise  *)
(* holds until the consumer lowers it.                                     *)
(*                                                                         *)
(* TLA+ steps are sequentially consistent, so this is sound only where the *)
(* code realizes each step atomically: the publish and the test of the     *)
(* record, and the clear and the check, are separated by a sequentially    *)
(* consistent fence in the code, which a checker of the Rust memory model  *)
(* checks below this spec. TLC checks that this module refines             *)
(* DoorbellContract.tla, and the controls each break one rule and must     *)
(* fail it:                                                                *)
(*                                                                         *)
(*   "clear-before-reset"       the consumer clears the record before it   *)
(*                              lowers the signal, so a producer that      *)
(*                              rings in between is lowered and its        *)
(*                              record stays set                           *)
(*   "skip-check"               the consumer waits after clearing without  *)
(*                              checking again, so work published before   *)
(*                              the clear is never rung for                *)
(*   "check-before-clear"       the consumer checks before it clears, so   *)
(*                              work published between its check and its   *)
(*                              clear finds the record set and rings for   *)
(*                              nothing                                    *)
(*   "test-before-publish"      a producer tests the record before it      *)
(*                              publishes, so its test can find the        *)
(*                              record set by a ring the consumer has      *)
(*                              already taken                              *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS
    Producers,   \* the producers ringing this doorbell
    MaxPublish,  \* how many items of work each producer publishes
    Control      \* "none", or the rule a negative control breaks

Controls == {"none", "clear-before-reset", "skip-check", "check-before-clear",
             "test-before-publish"}

ASSUME Producers # {} /\ MaxPublish \in Nat /\ Control \in Controls

Total == Cardinality(Producers) * MaxPublish

VARIABLES
    published,  \* how much work has been published, in all
    taken,      \* how much of it the consumer has taken
    rung,       \* the record of being rung
    raised,     \* the signal's level
    sent,       \* how much work each producer has published
    ppc,        \* where each producer is: "idle", "publish", "test" or "raise"
    raising,    \* whether a producer that tested before publishing found the record clear
    cpc,        \* where the consumer is
    skips       \* how many rings found the record set and skipped the raise

vars == <<published, taken, rung, raised, sent, ppc, raising, cpc, skips>>

ConsumerSteps == {"take", "reset", "clear", "check", "retake", "wait"}

\* Audience: internal.
\* Every variable holds a value of its kind.
TypeOK ==
    /\ published \in 0..Total
    /\ taken \in 0..published
    /\ rung \in BOOLEAN
    /\ raised \in BOOLEAN
    /\ sent \in [Producers -> 0..MaxPublish]
    /\ ppc \in [Producers -> {"idle", "publish", "test", "raise"}]
    /\ raising \in [Producers -> BOOLEAN]
    /\ cpc \in ConsumerSteps
    /\ skips \in 0..Total

Init ==
    /\ published = 0
    /\ taken = 0
    /\ rung = FALSE
    /\ raised = FALSE
    /\ sent = [p \in Producers |-> 0]
    /\ ppc = [p \in Producers |-> "idle"]
    /\ raising = [p \in Producers |-> FALSE]
    /\ cpc = "take"
    /\ skips = 0

-----------------------------------------------------------------------------
(* Producers.                                                              *)

\* The producer's swap of the record to set; it raises only if the record was clear.
Swap(p, next) ==
    /\ rung' = TRUE
    /\ skips' = IF rung THEN skips + 1 ELSE skips
    /\ ppc' = [ppc EXCEPT ![p] = IF rung THEN next ELSE "raise"]

\* Publishes one item: the queue's store of the entry, before the fence.
Publish(p) ==
    /\ Control # "test-before-publish"
    /\ ppc[p] = "idle"
    /\ sent[p] < MaxPublish
    /\ published' = published + 1
    /\ sent' = [sent EXCEPT ![p] = @ + 1]
    /\ ppc' = [ppc EXCEPT ![p] = "test"]
    /\ UNCHANGED <<taken, rung, raised, raising, cpc, skips>>

\* Tests the record, after the fence that orders it behind the publish.
Test(p) ==
    /\ Control # "test-before-publish"
    /\ ppc[p] = "test"
    /\ Swap(p, "idle")
    /\ UNCHANGED <<published, taken, raised, sent, raising, cpc>>

\* The control's producer, which tests before it publishes and remembers what it found.
TestFirst(p) ==
    /\ Control = "test-before-publish"
    /\ ppc[p] = "idle"
    /\ sent[p] < MaxPublish
    /\ rung' = TRUE
    /\ raising' = [raising EXCEPT ![p] = ~rung]
    /\ skips' = IF rung THEN skips + 1 ELSE skips
    /\ ppc' = [ppc EXCEPT ![p] = "publish"]
    /\ UNCHANGED <<published, taken, raised, sent, cpc>>

PublishAfterTest(p) ==
    /\ ppc[p] = "publish"
    /\ published' = published + 1
    /\ sent' = [sent EXCEPT ![p] = @ + 1]
    /\ ppc' = [ppc EXCEPT ![p] = IF raising[p] THEN "raise" ELSE "idle"]
    /\ UNCHANGED <<taken, rung, raised, raising, cpc, skips>>

\* Raises the signal: `Signal::raise`, the system call a set record lets a ring skip.
Raise(p) ==
    /\ ppc[p] = "raise"
    /\ raised' = TRUE
    /\ ppc' = [ppc EXCEPT ![p] = "idle"]
    /\ UNCHANGED <<published, taken, rung, sent, raising, cpc, skips>>

ProducerStep(p) ==
    Publish(p) \/ Test(p) \/ TestFirst(p) \/ PublishAfterTest(p) \/ Raise(p)

-----------------------------------------------------------------------------
(* The consumer.                                                           *)

\* The consumer's step that follows `from`, in the order the protocol, or the control,
\* puts them: take, lower, clear, check.
After(from) ==
    CASE Control = "clear-before-reset" ->
            ("take" :> "clear" @@ "clear" :> "reset" @@ "reset" :> "check")[from]
      [] Control = "check-before-clear" ->
            ("take" :> "reset" @@ "reset" :> "check" @@ "clear" :> "wait")[from]
      [] OTHER ->
            ("take" :> "reset" @@ "reset" :> "clear" @@ "clear" :> "check")[from]

\* Takes everything published: the consumer's drain of the queue.
Take ==
    /\ cpc = "take"
    /\ taken' = published
    /\ cpc' = After("take")
    /\ UNCHANGED <<published, rung, raised, sent, ppc, raising, skips>>

\* Lowers the signal: `Signal::lower`.
Reset ==
    /\ cpc = "reset"
    /\ raised' = FALSE
    /\ cpc' = After("reset")
    /\ UNCHANGED <<published, taken, rung, sent, ppc, raising, skips>>

\* Clears the record of being rung, before the fence that orders the check behind it.
Clear ==
    /\ cpc = "clear"
    /\ rung' = FALSE
    /\ cpc' = IF Control = "skip-check" THEN "wait" ELSE After("clear")
    /\ UNCHANGED <<published, taken, raised, sent, ppc, raising, skips>>

\* Checks again for work. Under "check-before-clear" an empty queue goes on to clear.
Check ==
    /\ cpc = "check"
    /\ cpc' = IF taken < published THEN "retake"
              ELSE IF Control = "check-before-clear" THEN "clear" ELSE "wait"
    /\ UNCHANGED <<published, taken, rung, raised, sent, ppc, raising, skips>>

\* Takes what the check found, and checks again, lowering and clearing nothing.
Retake ==
    /\ cpc = "retake"
    /\ taken' = published
    /\ cpc' = "check"
    /\ UNCHANGED <<published, rung, raised, sent, ppc, raising, skips>>

\* Woken by the raised signal: `Signal::wait` returning true.
Wake ==
    /\ cpc = "wait"
    /\ raised
    /\ cpc' = "take"
    /\ UNCHANGED <<published, taken, rung, raised, sent, ppc, raising, skips>>

\* Woken with the signal lowered: a pumped wait's bound passing, or a spurious wake.
WakeSpuriously ==
    /\ cpc = "wait"
    /\ cpc' = "take"
    /\ UNCHANGED <<published, taken, rung, raised, sent, ppc, raising, skips>>

ConsumerStep == Take \/ Reset \/ Clear \/ Check \/ Retake

Next ==
    \/ \E p \in Producers : ProducerStep(p)
    \/ ConsumerStep \/ Wake \/ WakeSpuriously

Spec == Init /\ [][Next]_vars

\* Each producer finishes what it began, the consumer moves on from every step but its
\* wait, and a raised signal wakes it. A spurious wake is never owed.
FairSpec ==
    /\ Spec
    /\ \A p \in Producers : WF_vars(ProducerStep(p))
    /\ WF_vars(ConsumerStep)
    /\ WF_vars(Wake)

-----------------------------------------------------------------------------
(* The contract, under the mapping.                                        *)

Contract ==
    INSTANCE DoorbellContract WITH
        Total <- Total,
        consumer <- IF cpc = "wait" THEN "waiting" ELSE "running"

\* Audience: internal.
\* Every behaviour of the mechanism is one of the contract's under the mapping: work is
\* published and taken only as the contract allows, and the consumer waits and wakes only
\* as it does. It is Contract!Spec written out, so that TLC names a violation by a line of
\* this definition.
ContractRefined ==
    /\ Contract!Init
    /\ [][Contract!Next]_<<published, taken, IF cpc = "wait" THEN "waiting" ELSE "running">>

\* Audience: internal.
\* The contract's guarantee that every item of work published is taken, under the mapping.
WorkIsTaken == Contract!WorkIsTaken

\* Audience: internal.
\* The contract's guarantee that a consumer waiting while work is there is woken, under the
\* mapping.
AWaiterIsWoken == Contract!AWaiterIsWoken

-----------------------------------------------------------------------------
(* Witnesses.                                                              *)

\* A ring found the record set and skipped its raise.
Witness_ARingSkipsItsRaise == skips = 0

\* The consumer waits with every producer done and nothing left to take.
Witness_TheConsumerWaitsWhenAllIsTaken ==
    ~(cpc = "wait" /\ taken = Total /\ \A p \in Producers : ppc[p] = "idle")

\* The consumer took work without lowering the signal or clearing the record again.
Witness_ABusyConsumerTakesAgainWithoutASystemCall == cpc # "retake"

=============================================================================
