\* Copyright (c) Michael Grier.
---------------------- MODULE ClientContractCheck ----------------------
(***************************************************************************
 * Negative controls for ClientContract. The control constant and mutations
 * are deliberately outside the public module a dependent instantiates.
 ***************************************************************************)
EXTENDS ClientContract

CONSTANT Control

ASSUME Control \in {"duplicate-delivery", "known-loss", "out-of-order", "overfill"}

FaultDuplicateDelivery(i) ==
    /\ Control = "duplicate-delivery"
    /\ i \in DOMAIN submissions
    /\ submissions[i].state = "delivered"
    /\ submissions[i].deliveries = 1
    /\ submissions' = [submissions EXCEPT ![i] =
        [submissions[i] EXCEPT !.deliveries = 2]]
    /\ UNCHANGED <<sessionState, pairState, contents, recoveryRequired>>

FaultLoseOutstanding(pairSet) ==
    [i \in DOMAIN submissions |->
        IF submissions[i].pair \in pairSet
           /\ submissions[i].state \in {"pending", "complete"}
        THEN [submissions[i] EXCEPT !.state = "unavailable"]
        ELSE submissions[i]]

FaultClosePair(p) ==
    /\ Control = "known-loss"
    /\ p \in LivePairs
    /\ pairState' = [pairState EXCEPT ![p] = "closed"]
    /\ submissions' = FaultLoseOutstanding({p})
    /\ UNCHANGED <<sessionState, contents, recoveryRequired>>

FaultComplete(i, result, effect, value, needsRecovery) ==
    /\ Control = "out-of-order"
    /\ i \in DOMAIN submissions
    /\ submissions[i].state = "pending"
    /\ submissions[i].pair \in LivePairs
    /\ result \in Results
    /\ effect \in EffectClasses
    /\ value \in Values
    /\ needsRecovery \in BOOLEAN
    /\ submissions' = [submissions EXCEPT ![i] =
        [submissions[i] EXCEPT
            !.state = "complete",
            !.result = result,
            !.effect = effect]]
    /\ contents' = IF effect = DefinitelyNoEffect THEN contents ELSE value
    /\ recoveryRequired' = IF effect = EffectUnknown THEN needsRecovery ELSE FALSE
    /\ UNCHANGED <<sessionState, pairState>>

FaultSubmit(p, command) ==
    /\ Control = "overfill"
    /\ Len(submissions) < MaxSubmissions
    /\ p \in Pairs
    /\ command \in Commands
    /\ sessionState = "live"
    /\ p \in LivePairs
    /\ Accepted(p, command)
    /\ UNCHANGED <<sessionState, pairState, contents, recoveryRequired>>

FaultNext ==
    \/ Next
    \/ \E i \in DOMAIN submissions : FaultDuplicateDelivery(i)
    \/ \E p \in Pairs : FaultClosePair(p)
    \/ \E i \in DOMAIN submissions, result \in Results,
          effect \in EffectClasses, value \in Values, needsRecovery \in BOOLEAN :
            FaultComplete(i, result, effect, value, needsRecovery)
    \/ \E p \in Pairs, command \in Commands : FaultSubmit(p, command)

CheckSpec == Init /\ [][FaultNext]_vars

=============================================================================
