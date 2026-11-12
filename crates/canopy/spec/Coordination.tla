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
(*   CoordinationProperties  writer exclusion, the count, reads            *)
(*   CoordinationAttachmentProperties  the gate, registry, clones, leases  *)
(*   CoordinationTransactionProperties transactions, recovery, discovery   *)
(*   CoordinationUpgradeProperties     the upgrade body, reader schemas    *)
(*   CoordinationWitnesses   properties a reachable state or step violates *)
(*   CoordinationLiveness    fairness, progress and the cyclic witnesses   *)
(***************************************************************************)
EXTENDS CoordinationLiveness
=============================================================================
