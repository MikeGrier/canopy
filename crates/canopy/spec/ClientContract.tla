\* Copyright (c) Michael Grier.
------------------------- MODULE ClientContract -------------------------
(***************************************************************************
 * The opaque behavior a canopy client can compose with. It knows commands,
 * local request correlation, logical contents and recovery state. It does
 * not know the ring, coordination, publication or filesystem mechanism.
 ***************************************************************************)
EXTENDS Naturals, FiniteSets, Sequences

CONSTANTS
    Pairs,
    Commands,
    Results,
    Values,
    InitialValue,
    CompletionCapacity,
    MaxSubmissions

ASSUME Pairs # {}
ASSUME Commands # {}
ASSUME Results # {}
ASSUME Values # {}
ASSUME InitialValue \in Values
ASSUME CompletionCapacity \in Nat \ {0}
ASSUME MaxSubmissions \in Nat

None == "none"
NoPair == 0
DefinitelyNoEffect == "DefinitelyNoEffect"
DefinitelyApplied == "DefinitelyApplied"
EffectUnknown == "EffectUnknown"
EffectClasses == {DefinitelyNoEffect, DefinitelyApplied, EffectUnknown}
PairClosed == "PairClosed"
CompletionQueueFull == "CompletionQueueFull"

VARIABLES
    sessionState,
    pairState,
    submissions,
    contents,
    recoveryRequired

vars == <<sessionState, pairState, submissions, contents, recoveryRequired>>

Init ==
    /\ sessionState = "new"
    /\ pairState = [p \in Pairs |-> "new"]
    /\ submissions = <<>>
    /\ contents = InitialValue
    /\ recoveryRequired = FALSE

OpenSession ==
    /\ sessionState = "new"
    /\ sessionState' = "live"
    /\ UNCHANGED <<pairState, submissions, contents, recoveryRequired>>

OpenPair(p) ==
    /\ sessionState = "live"
    /\ p \in Pairs
    /\ pairState[p] = "new"
    /\ pairState' = [pairState EXCEPT ![p] = "live"]
    /\ UNCHANGED <<sessionState, submissions, contents, recoveryRequired>>

LivePairs == {p \in Pairs : pairState[p] = "live"}

NextRequestId(p) ==
    1 + Cardinality({i \in DOMAIN submissions :
        submissions[i].pair = p /\ submissions[i].state # "refused"})

Outstanding(p) ==
    Cardinality({i \in DOMAIN submissions :
        submissions[i].pair = p /\ submissions[i].state \in {"pending", "complete"}})

Accepted(p, command) ==
    submissions' = Append(submissions,
        [pair |-> p, command |-> command, request |-> NextRequestId(p),
         state |-> "pending", refusal |-> None, result |-> None,
         effect |-> None, settled |-> FALSE, deliveries |-> 0])

Refused(p, command, reason) ==
    submissions' = Append(submissions,
        [pair |-> p, command |-> command, request |-> 0,
         state |-> "refused", refusal |-> reason, result |-> None,
         effect |-> None, settled |-> FALSE, deliveries |-> 0])

Submit(p, command) ==
    /\ Len(submissions) < MaxSubmissions
    /\ p \in Pairs
    /\ command \in Commands
    /\ IF sessionState # "live" \/ p \notin LivePairs
       THEN Refused(p, command, PairClosed)
       ELSE IF Outstanding(p) >= CompletionCapacity
       THEN Refused(p, command, CompletionQueueFull)
       ELSE Accepted(p, command)
    /\ UNCHANGED <<sessionState, pairState, contents, recoveryRequired>>

IsNext(p, request) ==
    \A j \in DOMAIN submissions :
        submissions[j].pair = p /\ submissions[j].state = "pending"
            => request <= submissions[j].request

Complete(i, result, effect, value, needsRecovery) ==
    /\ i \in DOMAIN submissions
    /\ submissions[i].state = "pending"
    /\ submissions[i].pair \in LivePairs
    /\ IsNext(submissions[i].pair, submissions[i].request)
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

CompleteAny ==
    \E i \in DOMAIN submissions, result \in Results,
       effect \in EffectClasses, value \in Values, needsRecovery \in BOOLEAN :
        Complete(i, result, effect, value, needsRecovery)

Deliver(i) ==
    /\ i \in DOMAIN submissions
    /\ submissions[i].state = "complete"
    /\ submissions[i].pair \in LivePairs
    /\ submissions' = [submissions EXCEPT ![i] =
        [submissions[i] EXCEPT !.state = "delivered", !.deliveries = 1]]
    /\ UNCHANGED <<sessionState, pairState, contents, recoveryRequired>>

DeliverAny == \E i \in DOMAIN submissions : Deliver(i)

LoseOutstanding(pairSet) ==
    [i \in DOMAIN submissions |->
        IF submissions[i].pair \in pairSet
           /\ submissions[i].state \in {"pending", "complete"}
        THEN [submissions[i] EXCEPT
                !.state = "unavailable",
                !.effect = EffectUnknown,
                !.settled = FALSE]
        ELSE submissions[i]]

ResolveUnavailable(i, result, actualEffect, value, needsRecovery) ==
    /\ i \in DOMAIN submissions
    /\ submissions[i].state = "unavailable"
    /\ ~submissions[i].settled
    /\ result \in Results
    /\ actualEffect \in EffectClasses
    /\ value \in Values
    /\ needsRecovery \in BOOLEAN
    /\ submissions' = [submissions EXCEPT ![i] =
        [submissions[i] EXCEPT
            !.result = result,
            !.effect = EffectUnknown,
            !.settled = TRUE]]
    /\ contents' = IF actualEffect = DefinitelyNoEffect THEN contents ELSE value
    /\ recoveryRequired' = IF actualEffect = EffectUnknown THEN needsRecovery ELSE FALSE
    /\ UNCHANGED <<sessionState, pairState>>

ResolveUnavailableAny ==
    \E i \in DOMAIN submissions, result \in Results, actualEffect \in EffectClasses,
       value \in Values, needsRecovery \in BOOLEAN :
        ResolveUnavailable(i, result, actualEffect, value, needsRecovery)

ClosePair(p) ==
    /\ p \in LivePairs
    /\ pairState' = [pairState EXCEPT ![p] = "closed"]
    /\ submissions' = LoseOutstanding({p})
    /\ UNCHANGED <<sessionState, contents, recoveryRequired>>

CloseSession ==
    /\ sessionState = "live"
    /\ sessionState' = "closed"
    /\ pairState' = [p \in Pairs |->
        IF p \in LivePairs THEN "closed" ELSE pairState[p]]
    /\ submissions' = LoseOutstanding(LivePairs)
    /\ UNCHANGED <<contents, recoveryRequired>>

Recover ==
    /\ recoveryRequired
    /\ recoveryRequired' = FALSE
    /\ UNCHANGED <<sessionState, pairState, submissions, contents>>

Next ==
    \/ OpenSession
    \/ \E p \in Pairs : OpenPair(p)
    \/ \E p \in Pairs, command \in Commands : Submit(p, command)
    \/ CompleteAny
    \/ DeliverAny
    \/ ResolveUnavailableAny
    \/ \E p \in Pairs : ClosePair(p)
    \/ CloseSession
    \/ Recover

ClientSpec == Init /\ [][Next]_vars
ClientLiveSpec == ClientSpec /\ WF_vars(CompleteAny) /\ WF_vars(DeliverAny)

\* Audience: internal.
TypeOK ==
    /\ sessionState \in {"new", "live", "closed"}
    /\ pairState \in [Pairs -> {"new", "live", "closed"}]
    /\ sessionState # "live" => LivePairs = {}
    /\ submissions \in Seq([pair : Pairs \cup {NoPair},
                            command : Commands \cup {None},
                            request : Nat,
                            state : {"pending", "complete", "delivered",
                                     "unavailable", "refused", None},
                            refusal : {PairClosed, CompletionQueueFull, None},
                            result : Results \cup {None},
                            effect : EffectClasses \cup {None},
                            settled : BOOLEAN,
                            deliveries : Nat])
    /\ contents \in Values
    /\ recoveryRequired \in BOOLEAN

\* Audience: client. Refusal and acceptance are distinct submission results. Shows through: submission outcome.
SubmissionOutcomesCoherent ==
    \A i \in DOMAIN submissions :
        IF submissions[i].state = "refused"
        THEN submissions[i].request = 0
             /\ submissions[i].refusal \in {PairClosed, CompletionQueueFull}
        ELSE submissions[i].request \in Nat \ {0}
             /\ submissions[i].refusal = None

\* Audience: client. An accepted request produces at most one delivered completion. Shows through: completion delivery.
AtMostOneDelivery ==
    \A i \in DOMAIN submissions : submissions[i].deliveries <= 1

\* Audience: client. A later request cannot complete before an earlier pending one. Shows through: completion order.
PerPairCompletionOrder ==
    \A i, j \in DOMAIN submissions :
        /\ submissions[i].pair = submissions[j].pair
        /\ submissions[i].request < submissions[j].request
        /\ submissions[j].state \in {"complete", "delivered"}
        => submissions[i].state # "pending"

\* Audience: client. Pair loss never claims no effect. Shows through: unavailable completion and effect certainty.
LostCompletionIsUnknown ==
    \A i \in DOMAIN submissions :
        submissions[i].state = "unavailable" => submissions[i].effect = EffectUnknown

\* Audience: client. Submission reserves completion capacity. Shows through: accepted or refused submission.
CompletionCapacityReserved ==
    \A p \in Pairs : Outstanding(p) <= CompletionCapacity

\* Audience: client. Recovery is required only with uncertain effect. Shows through: recovery-required state.
RecoveryReportsUncertainEffect ==
    recoveryRequired => \E i \in DOMAIN submissions : submissions[i].effect = EffectUnknown

Witness_AnAcceptedCompletion ==
    ~\E i \in DOMAIN submissions : submissions[i].state \in {"complete", "delivered"}

Witness_ARefusalBecauseThePairIsClosed ==
    ~\E i \in DOMAIN submissions : submissions[i].state = "refused"
                                 /\ submissions[i].refusal = PairClosed

Witness_ARefusalBecauseTheCompletionQueueIsFull ==
    ~\E i \in DOMAIN submissions : submissions[i].state = "refused"
                                 /\ submissions[i].refusal = CompletionQueueFull

Witness_ADefiniteNoEffectCompletion ==
    ~\E i \in DOMAIN submissions : submissions[i].state \in {"complete", "delivered"}
                                 /\ submissions[i].effect = DefinitelyNoEffect

Witness_ADefiniteAppliedCompletion ==
    ~\E i \in DOMAIN submissions : submissions[i].state \in {"complete", "delivered"}
                                 /\ submissions[i].effect = DefinitelyApplied

Witness_ACompletionWithUnknownEffect ==
    ~\E i \in DOMAIN submissions : submissions[i].state \in {"complete", "delivered"}
                                 /\ submissions[i].effect = EffectUnknown

Witness_PairLossWithNoVisibleEffect ==
    ~\E i \in DOMAIN submissions : submissions[i].state = "unavailable"
                                 /\ contents = InitialValue
                                 /\ ~recoveryRequired

Witness_PairLossAfterADefiniteEffect ==
    ~\E i \in DOMAIN submissions : submissions[i].state = "unavailable"
                                 /\ contents # InitialValue
                                 /\ ~recoveryRequired

Witness_PairLossWithAnUnknownEffect ==
    ~\E i \in DOMAIN submissions : submissions[i].state = "unavailable"
                                 /\ recoveryRequired

=============================================================================
