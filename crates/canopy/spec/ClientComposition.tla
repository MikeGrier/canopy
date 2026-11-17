\* Copyright (c) Michael Grier.
----------------------- MODULE ClientComposition -------------------------
(***************************************************************************)
(* Composes Ring with the store's observable operation outcomes and projects *)
(* both into ClientContract. Ring contributes identity, order, withdrawal   *)
(* and completion availability; the store contributes contents and recovery.*)
(* No variable named started is present in the projected client history.     *)
(***************************************************************************)
EXTENDS Ring

CONSTANTS
    Commands,
    Results,
    Values,
    InitialValue,
    Control

ASSUME Commands # {}
ASSUME Results # {}
ASSUME Values # {}
ASSUME InitialValue \in Values
ASSUME Control \in {"none", "outcome", "order", "ambiguity", "ready-pair"}

NoneClient == "none"
NoPair == 0
DefinitelyNoEffect == "DefinitelyNoEffect"
DefinitelyApplied == "DefinitelyApplied"
EffectUnknown == "EffectUnknown"
EffectClasses == {DefinitelyNoEffect, DefinitelyApplied, EffectUnknown}

VARIABLES
    clientOutcomes,
    clientContents,
    clientRecovery

compositionVars == <<vars, clientOutcomes, clientContents, clientRecovery>>

PendingOutcome == [effect |-> NoneClient, result |-> NoneClient, settled |-> FALSE]

CompositionInit ==
    /\ Init
    /\ clientOutcomes = <<>>
    /\ clientContents = InitialValue
    /\ clientRecovery = FALSE

ProjectedPairState == [p \in Pairs |-> IF live[p] THEN "live" ELSE "closed"]

ProjectedState(request) ==
    CASE request.state = "refused" -> "refused"
      [] request.state = "delivered" -> "delivered"
      [] request.state = "withdrawn" -> "unavailable"
      [] request.state = "unavailable" -> "unavailable"
      [] ~live[request.pair] -> "unavailable"
      [] request.state \in {"queued", "running"} -> "pending"
      [] OTHER -> "complete"

ClientProjectedEntry(i) ==
    LET request == requests[i]
        state == ProjectedState(request)
        outcome == clientOutcomes[i]
    IN [pair |-> request.pair,
        command |-> request.command,
        request |-> request.request,
        state |-> state,
        refusal |-> request.refusal,
        result |-> outcome.result,
        effect |-> IF state = "unavailable"
                   THEN IF Control = "ambiguity" THEN DefinitelyNoEffect ELSE EffectUnknown
                   ELSE outcome.effect,
        settled |-> state = "unavailable" /\ outcome.settled,
        deliveries |-> IF state = "delivered" THEN 1 ELSE 0]

ProjectedSubmissions == [i \in DOMAIN requests |-> ClientProjectedEntry(i)]

Client == INSTANCE ClientContract WITH
    Pairs <- Pairs,
    Commands <- Commands,
    Results <- Results,
    Values <- Values,
    InitialValue <- InitialValue,
    CompletionCapacity <- CompletionCapacity,
    MaxSubmissions <- MaxRequests,
    sessionState <- "live",
    pairState <- ProjectedPairState,
    submissions <- ProjectedSubmissions,
    contents <- clientContents,
    recoveryRequired <- clientRecovery

Toy == INSTANCE ToyClient WITH
    Pairs <- Pairs,
    Commands <- Commands,
    Results <- Results,
    Values <- Values,
    InitialValue <- InitialValue,
    CompletionCapacity <- CompletionCapacity,
    MaxSubmissions <- MaxRequests,
    sessionState <- "live",
    pairState <- ProjectedPairState,
    submissions <- ProjectedSubmissions,
    applicationContents <- clientContents,
    recoveryRequired <- clientRecovery

PushStep ==
    /\ Next
    /\ Len(requests') = Len(requests) + 1
    /\ clientOutcomes' = Append(clientOutcomes, PendingOutcome)
    /\ UNCHANGED <<clientContents, clientRecovery>>

FinishStep ==
    /\ Next
    /\ running # NoRequest
    /\ running' = NoRequest
    /\ LET i == running
           p == requests[i].pair
       IN IF live[p]
          THEN \E result \in Results, effect \in EffectClasses,
                  value \in Values, needsRecovery \in BOOLEAN :
                /\ clientOutcomes' = [clientOutcomes EXCEPT ![i] =
                    [effect |-> IF Control = "outcome" THEN DefinitelyNoEffect ELSE effect,
                     result |-> result,
                     settled |-> TRUE]]
                /\ clientContents' = IF Control = "outcome"
                                     THEN value
                                     ELSE IF effect = DefinitelyNoEffect
                                     THEN clientContents
                                     ELSE value
                /\ clientRecovery' = IF Control = "outcome"
                                     THEN FALSE
                                     ELSE IF effect = EffectUnknown
                                     THEN needsRecovery
                                     ELSE FALSE
          ELSE \E result \in Results, actualEffect \in EffectClasses,
                  value \in Values, needsRecovery \in BOOLEAN :
                /\ clientOutcomes' = [clientOutcomes EXCEPT ![i] =
                    [effect |-> EffectUnknown, result |-> result, settled |-> TRUE]]
                /\ clientContents' = IF actualEffect = DefinitelyNoEffect
                                     THEN clientContents
                                     ELSE value
                /\ clientRecovery' = IF actualEffect = EffectUnknown
                                     THEN needsRecovery
                                     ELSE FALSE

DropStep ==
    /\ Next
    /\ \E p \in Pairs : live[p] /\ ~live'[p]
    /\ clientOutcomes' = [i \in DOMAIN clientOutcomes |->
        IF requests[i].pair \in {p \in Pairs : live[p] /\ ~live'[p]}
        THEN [clientOutcomes[i] EXCEPT !.settled = FALSE]
        ELSE clientOutcomes[i]]
    /\ UNCHANGED <<clientContents, clientRecovery>>

OtherStep ==
    /\ Next
    /\ Len(requests') = Len(requests)
    /\ ~(running # NoRequest /\ running' = NoRequest)
    /\ live' = live
    /\ UNCHANGED <<clientOutcomes, clientContents, clientRecovery>>

RecoverClient ==
    /\ clientRecovery
    /\ clientRecovery' = FALSE
    /\ UNCHANGED <<vars, clientOutcomes, clientContents>>

FaultStartOutOfOrder(p) ==
    /\ Control = "order"
    /\ p \in Pairs
    /\ running = NoRequest
    /\ Ready(p)
    /\ \E i \in QueuedIds(p), j \in QueuedIds(p) :
        /\ requests[j].request < requests[i].request
        /\ requests' = [requests EXCEPT ![i] =
            [requests[i] EXCEPT !.state = "running", !.started = TRUE]]
        /\ running' = i
        /\ announced' = [announced EXCEPT ![p] = TRUE]
        /\ cursor' = NextPair(p)
        /\ served' = [served EXCEPT ![p] = TRUE]
        /\ skipped' = {}
        /\ UNCHANGED <<live, nextRequest, contractRequests, submissionPublished,
                       submissionTaken, notifications, submissionConsumer>>
        /\ UNCHANGED completionVars
    /\ UNCHANGED <<clientOutcomes, clientContents, clientRecovery>>

FaultSkipReadyPair(p) ==
    /\ Control = "ready-pair"
    /\ p \in Pairs
    /\ running = NoRequest
    /\ Ready(p)
    /\ \E q \in Pairs : Ready(q) /\ Distance(cursor, q) < Distance(cursor, p)
    /\ LET i == EarliestQueued(p)
       IN /\ requests' = [requests EXCEPT ![i] =
            [requests[i] EXCEPT !.state = "running", !.started = TRUE]]
          /\ running' = i
          /\ announced' = [announced EXCEPT ![p] = Cardinality(QueuedIds(p)) > 1]
    /\ cursor' = NextPair(p)
    /\ served' = [served EXCEPT ![p] = TRUE]
    /\ skipped' = {q \in Pairs : Ready(q) /\ Distance(cursor, q) < Distance(cursor, p)}
    /\ UNCHANGED <<live, nextRequest, contractRequests, submissionPublished,
                   submissionTaken, notifications, submissionConsumer>>
    /\ UNCHANGED completionVars
    /\ UNCHANGED <<clientOutcomes, clientContents, clientRecovery>>

CompositionNext ==
    \/ PushStep
    \/ FinishStep
    \/ DropStep
    \/ OtherStep
    \/ RecoverClient

CompositionCheckNext ==
    \/ CompositionNext
    \/ \E p \in Pairs : FaultStartOutOfOrder(p) \/ FaultSkipReadyPair(p)

CompositionSpec == CompositionInit /\ [][CompositionNext]_compositionVars
CompositionCheckSpec == CompositionInit /\ [][CompositionCheckNext]_compositionVars
CompositionFairSpec ==
    /\ CompositionSpec
    /\ WF_compositionVars(FinishStep)
    /\ \A p \in Pairs : WF_compositionVars(TakeCompletions(p))
    /\ WF_compositionVars(RecoverClient)

ClientProjectionVars == <<ProjectedPairState, ProjectedSubmissions,
                          clientContents, clientRecovery>>

ClientContractInitialized ==
    /\ ProjectedPairState = [p \in Pairs |-> "live"]
    /\ ProjectedSubmissions = <<>>
    /\ clientContents = InitialValue
    /\ ~clientRecovery

\* Audience: internal.
ClientRefined == ClientContractInitialized /\ [][Client!Next]_ClientProjectionVars

\* Audience: internal.
CompositionTypeOK ==
    /\ TypeOK
    /\ Len(clientOutcomes) = Len(requests)
    /\ clientContents \in Values
    /\ clientRecovery \in BOOLEAN
    /\ clientOutcomes \in Seq([effect : EffectClasses \cup {NoneClient},
                               result : Results \cup {NoneClient},
                               settled : BOOLEAN])

\* Audience: client. Mechanism-only start state is absent. Shows through: request history.
ProjectionHidesStarted ==
    \A i \in DOMAIN ProjectedSubmissions : "started" \notin DOMAIN ProjectedSubmissions[i]

\* Audience: client. Projected histories retain every client contract promise. Shows through: submissions and recovery.
ComposedClientProperties ==
    /\ Client!TypeOK
    /\ Client!SubmissionOutcomesCoherent
    /\ Client!AtMostOneDelivery
    /\ Client!PerPairCompletionOrder
    /\ Client!LostCompletionIsUnknown
    /\ Client!CompletionCapacityReserved
    /\ Client!RecoveryReportsUncertainEffect

\* Audience: client. The toy dependent sees the same client properties. Shows through: dependent state.
ToyDependentUnchanged ==
    /\ Toy!ApplicationContentsArePublic
    /\ Toy!ApplicationCannotDuplicateACompletion

Witness_UnavailableCompletion ==
    ~\E i \in DOMAIN ProjectedSubmissions : ProjectedSubmissions[i].state = "unavailable"

Witness_UnavailableSettlementWithRecovery ==
    ~\E i \in DOMAIN ProjectedSubmissions :
        /\ ProjectedSubmissions[i].state = "unavailable"
        /\ ProjectedSubmissions[i].settled
        /\ ProjectedSubmissions[i].effect = EffectUnknown
        /\ clientRecovery

=============================================================================
