\* Copyright (c) Michael Grier.
-------------------------- MODULE DoorbellContract --------------------------
(***************************************************************************)
(* The contract of a doorbell, stating only what its user observes         *)
(* (DESIGN-NOTES "The ring: a portable protocol over one platform          *)
(* primitive, behind a contract spec"). Producers publish work; a consumer *)
(* takes all of it that is published, and waits, and is woken. Work        *)
(* published is taken, and a consumer waiting while work is there to take  *)
(* is woken; a wake with nothing to take is allowed. Nothing about the     *)
(* record of being rung, the signal or the order of the consumer's steps   *)
(* is visible here: a spec above the doorbell composes with this module    *)
(* alone, and Doorbell.tla is checked to refine it.                        *)
(***************************************************************************)
EXTENDS Naturals

CONSTANT Total  \* how much work is published, in all

ASSUME Total \in Nat

VARIABLES
    published,  \* how much work has been published
    taken,      \* how much of it the consumer has taken
    consumer    \* "running", or "waiting" to be woken

vars == <<published, taken, consumer>>

TypeOK ==
    /\ published \in 0..Total
    /\ taken \in 0..published
    /\ consumer \in {"running", "waiting"}

Init == published = 0 /\ taken = 0 /\ consumer = "running"

\* A producer publishes one item of work.
Publish ==
    /\ published < Total
    /\ published' = published + 1
    /\ UNCHANGED <<taken, consumer>>

\* A running consumer takes everything published so far.
Take ==
    /\ consumer = "running"
    /\ taken' = published
    /\ UNCHANGED <<published, consumer>>

\* A running consumer waits, whether or not work is there to take.
Wait ==
    /\ consumer = "running"
    /\ consumer' = "waiting"
    /\ UNCHANGED <<published, taken>>

\* A waiting consumer is woken, with or without work to take.
Wake ==
    /\ consumer = "waiting"
    /\ consumer' = "running"
    /\ UNCHANGED <<published, taken>>

Next == Publish \/ Take \/ Wait \/ Wake

Spec == Init /\ [][Next]_vars

\* Audience: internal.
\* Every item of work published is taken: the consumer never leaves work behind for ever.
WorkIsTaken == \A n \in 1..Total : (published >= n) ~> (taken >= n)

\* Audience: internal.
\* A consumer waiting while work is there to take is woken.
AWaiterIsWoken == (consumer = "waiting" /\ taken < published) ~> (consumer = "running")

=============================================================================
