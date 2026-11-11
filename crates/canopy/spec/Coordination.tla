\* Copyright (c) Michael Grier.
---------------------------- MODULE Coordination ----------------------------
(***************************************************************************)
(* The coordination model, as the one module a configuration checks. Each  *)
(* module extends the one before it, and the checks and witnesses add to   *)
(* the model without touching it:                                          *)
(*   CoordinationState       constants, state, types and the initial state *)
(*   CoordinationAttachment  attaching, the gate and tiers, leases, registry *)
(*   CoordinationOperations  writer exclusion and reads                    *)
(*   CoordinationBehavior    death, crash, Next and Spec                   *)
(*   CoordinationProperties  invariants and action properties              *)
(*   CoordinationWitnesses   properties a reachable state or step violates *)
(***************************************************************************)
EXTENDS CoordinationWitnesses
=============================================================================
