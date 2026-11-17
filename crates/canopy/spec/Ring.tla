\* Copyright (c) Michael Grier.
-------------------------------- MODULE Ring --------------------------------
(***************************************************************************)
(* Queue pairs and their two doorbells. The submission producer queues an   *)
(* entry without ringing, then rings once for a batch. The engine is the     *)
(* only submission consumer and completion producer. It services ready       *)
(* pairs round-robin, parks a started request as the single in-flight work,   *)
(* and posts a completion without waiting for the client. A dropped pair      *)
(* withdraws queued entries and discards the completion of started work.      *)
(* The doorbells are used only through DoorbellContract.tla.                  *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, Sequences

CONSTANTS
    Pairs,
    CompletionCapacity,
    MaxRequests

ASSUME Pairs = {1, 2}
ASSUME CompletionCapacity \in Nat \ {0}
ASSUME MaxRequests \in Nat

PairClosed == "PairClosed"
CompletionQueueFull == "CompletionQueueFull"
None == "none"
NoRequest == 0
LastPair == Cardinality(Pairs)
MaxNotifications == MaxRequests

VARIABLES
    live,
    nextRequest,
    requests,
    contractRequests,
    announced,
    running,
    cursor,
    served,
    skipped,
    submissionPublished,
    submissionTaken,
    notifications,
    submissionConsumer,
    completion1Published,
    completion1Taken,
    completion1Consumer,
    completion2Published,
    completion2Taken,
    completion2Consumer

vars == <<live, nextRequest, requests, contractRequests, announced, running, cursor, served,
          skipped, submissionPublished, submissionTaken, notifications, submissionConsumer,
          completion1Published, completion1Taken, completion1Consumer,
          completion2Published, completion2Taken, completion2Consumer>>

completionVars == <<completion1Published, completion1Taken, completion1Consumer,
                    completion2Published, completion2Taken, completion2Consumer>>

SubmissionBell == INSTANCE DoorbellContract WITH
    Total <- MaxNotifications,
    published <- submissionPublished,
    taken <- submissionTaken,
    consumer <- submissionConsumer

CompletionBell1 == INSTANCE DoorbellContract WITH
    Total <- MaxRequests,
    published <- completion1Published,
    taken <- completion1Taken,
    consumer <- completion1Consumer

CompletionBell2 == INSTANCE DoorbellContract WITH
    Total <- MaxRequests,
    published <- completion2Published,
    taken <- completion2Taken,
    consumer <- completion2Consumer

CompletionPublish(p) == IF p = 1 THEN CompletionBell1!Publish ELSE CompletionBell2!Publish
CompletionTake(p) == IF p = 1 THEN CompletionBell1!Take ELSE CompletionBell2!Take
CompletionWait(p) == IF p = 1 THEN CompletionBell1!Wait ELSE CompletionBell2!Wait
CompletionWake(p) == IF p = 1 THEN CompletionBell1!Wake ELSE CompletionBell2!Wake

OtherCompletionBellUnchanged(p) ==
    IF p = 1
    THEN UNCHANGED <<completion2Published, completion2Taken, completion2Consumer>>
    ELSE UNCHANGED <<completion1Published, completion1Taken, completion1Consumer>>

CompletionTakenAt(p) ==
    IF p = 1 THEN completion1Taken ELSE completion2Taken

Init ==
    /\ live = [p \in Pairs |-> TRUE]
    /\ nextRequest = [p \in Pairs |-> 1]
    /\ requests = <<>>
    /\ contractRequests = <<>>
    /\ announced = [p \in Pairs |-> FALSE]
    /\ running = NoRequest
    /\ cursor = 1
    /\ served = [p \in Pairs |-> FALSE]
    /\ skipped = {}
    /\ SubmissionBell!Init
    /\ notifications = 0
    /\ completion1Published = 0
    /\ completion1Taken = 0
    /\ completion1Consumer = "running"
    /\ completion2Published = 0
    /\ completion2Taken = 0
    /\ completion2Consumer = "running"

Outstanding(p) ==
    Cardinality({i \in DOMAIN requests :
        requests[i].pair = p
        /\ requests[i].state \in {"queued", "running", "complete"}})

QueuedIds(p) ==
    {i \in DOMAIN requests :
        requests[i].pair = p /\ requests[i].state = "queued"}

PushAccepted(p) ==
    /\ requests' = Append(requests,
        [pair |-> p, command |-> "command", request |-> nextRequest[p],
         state |-> "queued", refusal |-> None, effect |-> FALSE,
         started |-> FALSE])
    /\ contractRequests' = Append(contractRequests,
        [pair |-> p, command |-> "command", request |-> nextRequest[p],
         state |-> "accepted", refusal |-> None])
    /\ nextRequest' = [nextRequest EXCEPT ![p] = @ + 1]
    /\ announced' = [announced EXCEPT ![p] = FALSE]
    /\ UNCHANGED <<live, running, cursor, served, skipped,
                   submissionPublished, submissionTaken, notifications, submissionConsumer>>
    /\ UNCHANGED completionVars

PushRefused(p, reason) ==
    /\ requests' = Append(requests,
        [pair |-> p, command |-> "command", request |-> 0,
         state |-> "refused", refusal |-> reason, effect |-> FALSE,
         started |-> FALSE])
    /\ contractRequests' = Append(contractRequests,
        [pair |-> p, command |-> "command", request |-> 0,
         state |-> "refused", refusal |-> reason])
    /\ UNCHANGED <<live, nextRequest, announced, running, cursor, served, skipped,
                   submissionPublished, submissionTaken, notifications, submissionConsumer>>
    /\ UNCHANGED completionVars

Push(p) ==
    /\ Len(requests) < MaxRequests
    /\ p \in Pairs
    /\ IF ~live[p]
       THEN PushRefused(p, PairClosed)
       ELSE IF Outstanding(p) >= CompletionCapacity
       THEN PushRefused(p, CompletionQueueFull)
       ELSE PushAccepted(p)

Notify(p) ==
    /\ p \in Pairs
    /\ live[p]
    /\ ~announced[p]
    /\ \E i \in DOMAIN requests : requests[i].pair = p
                                /\ requests[i].state = "queued"
    /\ SubmissionBell!Publish
    /\ notifications' = notifications + 1
    /\ announced' = [announced EXCEPT ![p] = TRUE]
    /\ UNCHANGED <<live, nextRequest, requests, contractRequests, running, cursor, served, skipped>>
    /\ UNCHANGED completionVars

Ready(p) ==
    /\ p \in Pairs
    /\ live[p]
    /\ announced[p]
    /\ QueuedIds(p) # {}
    /\ submissionTaken = submissionPublished
    /\ submissionConsumer = "running"

Distance(from, p) ==
    ((p - from + LastPair) % LastPair) + 1

IsNextReady(p) ==
    /\ Ready(p)
    /\ \A q \in Pairs : Ready(q) => Distance(cursor, p) <= Distance(cursor, q)

EarliestQueued(p) == CHOOSE i \in QueuedIds(p) :
    \A j \in QueuedIds(p) : requests[i].request <= requests[j].request

NextPair(p) == IF p = LastPair THEN 1 ELSE p + 1

Start(p) ==
    /\ running = NoRequest
    /\ IsNextReady(p)
    /\ LET i == EarliestQueued(p)
       IN /\ requests' = [requests EXCEPT ![i] =
            [requests[i] EXCEPT !.state = "running", !.started = TRUE]]
          /\ running' = i
          /\ announced' = [announced EXCEPT ![p] = Cardinality(QueuedIds(p)) > 1]
    /\ cursor' = NextPair(p)
    /\ served' = [served EXCEPT ![p] = TRUE]
    /\ skipped' = {}
    /\ UNCHANGED <<live, nextRequest, contractRequests, submissionPublished,
                   submissionTaken, notifications, submissionConsumer>>
    /\ UNCHANGED completionVars

Finish ==
    /\ running # NoRequest
    /\ LET i == running
           p == requests[i].pair
       IN IF live[p]
          THEN /\ CompletionPublish(p)
               /\ OtherCompletionBellUnchanged(p)
               /\ requests' = [requests EXCEPT ![i] =
                    [requests[i] EXCEPT !.state = "complete", !.effect = TRUE]]
               /\ contractRequests' = [contractRequests EXCEPT ![i] =
                    [contractRequests[i] EXCEPT !.state = "complete"]]
          ELSE /\ requests' = [requests EXCEPT ![i] =
                    [requests[i] EXCEPT !.state = "unavailable", !.effect = TRUE]]
               /\ UNCHANGED <<contractRequests, completionVars>>
    /\ running' = NoRequest
    /\ UNCHANGED <<live, nextRequest, announced, cursor, served, skipped,
                   submissionPublished, submissionTaken, notifications, submissionConsumer>>

TakeCompletions(p) ==
    /\ p \in Pairs
    /\ live[p]
    /\ \E i \in DOMAIN requests : requests[i].pair = p
                                /\ requests[i].state = "complete"
    /\ CompletionTake(p)
    /\ OtherCompletionBellUnchanged(p)
    /\ requests' = [i \in DOMAIN requests |->
        IF requests[i].pair = p /\ requests[i].state = "complete"
        THEN [requests[i] EXCEPT !.state = "delivered"]
        ELSE requests[i]]
    /\ contractRequests' = [i \in DOMAIN contractRequests |->
        IF contractRequests[i].pair = p /\ contractRequests[i].state = "complete"
        THEN [contractRequests[i] EXCEPT !.state = "delivered"]
        ELSE contractRequests[i]]
    /\ UNCHANGED <<live, nextRequest, announced, running, cursor, served, skipped,
                   submissionPublished, submissionTaken, notifications, submissionConsumer>>

DropPair(p) ==
    /\ p \in Pairs
    /\ live[p]
    /\ live' = [live EXCEPT ![p] = FALSE]
    /\ announced' = [announced EXCEPT ![p] = FALSE]
    /\ CompletionTake(p)
    /\ OtherCompletionBellUnchanged(p)
    /\ requests' = [i \in DOMAIN requests |->
        IF requests[i].pair # p THEN requests[i]
        ELSE IF requests[i].state = "queued"
        THEN [requests[i] EXCEPT !.state = "withdrawn"]
        ELSE IF requests[i].state = "complete"
        THEN [requests[i] EXCEPT !.state = "unavailable"]
        ELSE requests[i]]
    /\ contractRequests' = [i \in DOMAIN contractRequests |->
        IF contractRequests[i].pair = p
           /\ contractRequests[i].state \in {"accepted", "complete"}
        THEN [contractRequests[i] EXCEPT !.state = "unavailable"]
        ELSE contractRequests[i]]
    /\ UNCHANGED <<nextRequest, running, cursor, served, skipped,
                   submissionPublished, submissionTaken, notifications, submissionConsumer>>

TakeSubmissionBell == SubmissionBell!Take /\ UNCHANGED <<
    live, nextRequest, requests, contractRequests, announced, running, cursor,
    served, skipped, notifications>> /\ UNCHANGED completionVars

WaitSubmissionBell == SubmissionBell!Wait /\ UNCHANGED <<
    live, nextRequest, requests, contractRequests, announced, running, cursor,
    served, skipped, notifications>> /\ UNCHANGED completionVars

WakeSubmissionBell == SubmissionBell!Wake /\ UNCHANGED <<
    live, nextRequest, requests, contractRequests, announced, running, cursor,
    served, skipped, notifications>> /\ UNCHANGED completionVars

WaitCompletionBell(p) ==
    CompletionWait(p) /\ OtherCompletionBellUnchanged(p) /\ UNCHANGED <<
    live, nextRequest, requests, contractRequests, announced, running, cursor,
    served, skipped, submissionPublished, submissionTaken, notifications, submissionConsumer>>

WakeCompletionBell(p) ==
    CompletionWake(p) /\ OtherCompletionBellUnchanged(p) /\ UNCHANGED <<
    live, nextRequest, requests, contractRequests, announced, running, cursor,
    served, skipped, submissionPublished, submissionTaken, notifications, submissionConsumer>>

Next ==
    \/ \E p \in Pairs : Push(p) \/ Notify(p) \/ Start(p) \/ TakeCompletions(p)
       \/ DropPair(p) \/ WaitCompletionBell(p) \/ WakeCompletionBell(p)
    \/ Finish
    \/ TakeSubmissionBell
    \/ WaitSubmissionBell
    \/ WakeSubmissionBell

Spec == Init /\ [][Next]_vars
FairSpec ==
    /\ Spec
    /\ SF_vars(TakeSubmissionBell)
    /\ WF_vars(WakeSubmissionBell)
    /\ \A p \in Pairs : SF_vars(TakeCompletions(p) \/ DropPair(p))
    /\ \A p \in Pairs : WF_vars(WakeCompletionBell(p))

ProjectedEntry(request) ==
    [pair |-> request.pair,
     command |-> request.command,
     request |-> request.request,
     state |->
        CASE request.state = "refused" -> "refused"
          [] request.state = "delivered" -> "delivered"
          [] request.state = "withdrawn" -> "unavailable"
          [] request.state = "unavailable" -> "unavailable"
          [] ~live[request.pair] -> "unavailable"
          [] request.state \in {"queued", "running"} -> "accepted"
          [] OTHER -> "complete",
     refusal |-> request.refusal]

ProjectedRequests == [i \in DOMAIN requests |-> ProjectedEntry(requests[i])]

Contract == INSTANCE RingContract WITH
    Pairs <- Pairs,
    CompletionCapacity <- CompletionCapacity,
    MaxRequests <- MaxRequests,
    live <- live,
    nextRequest <- nextRequest,
    requests <- contractRequests

\* Audience: internal.
ContractRefined ==
    /\ Contract!Init
    /\ [][Contract!Next]_<<live, nextRequest, contractRequests>>

\* Audience: internal.
TypeOK ==
    /\ live \in [Pairs -> BOOLEAN]
    /\ nextRequest \in [Pairs -> Nat]
    /\ announced \in [Pairs -> BOOLEAN]
    /\ running \in 0..Len(requests)
    /\ cursor \in Pairs
    /\ served \in [Pairs -> BOOLEAN]
    /\ skipped \subseteq Pairs
    /\ submissionPublished \in 0..MaxNotifications
    /\ submissionTaken \in 0..submissionPublished
    /\ submissionConsumer \in {"running", "waiting"}
    /\ notifications \in 0..MaxNotifications
    /\ contractRequests = ProjectedRequests
    /\ contractRequests \in Seq([pair : Pairs,
                                  command : {"command"},
                                  request : Nat,
                                  state : {"accepted", "complete", "delivered",
                                           "unavailable", "refused"},
                                  refusal : {PairClosed, CompletionQueueFull, None}])
    /\ completion1Published \in 0..MaxRequests
    /\ completion1Taken \in 0..completion1Published
    /\ completion1Consumer \in {"running", "waiting"}
    /\ completion2Published \in 0..MaxRequests
    /\ completion2Taken \in 0..completion2Published
    /\ completion2Consumer \in {"running", "waiting"}

\* Audience: internal.
SubmissionNotificationsRung == notifications = submissionPublished

\* Audience: internal.
CompletionCapacityReserved ==
    \A p \in Pairs : Outstanding(p) <= CompletionCapacity

\* Audience: internal. A queued request withdrawn by drop never takes effect.
WithdrawnNeverTakesEffect ==
    \A i \in DOMAIN requests :
        requests[i].state = "withdrawn" => ~requests[i].effect /\ ~requests[i].started

\* Audience: internal. A completion posted for a live pair contains an effect and no blocker can hold the engine.
PostedCompletionHasEffect ==
    \A i \in DOMAIN requests :
        requests[i].state \in {"complete", "delivered"}
            => requests[i].effect

\* Audience: internal. Ready-pair service never skips an earlier ready pair in one cycle.
ReadyPairService == skipped = {}

\* Audience: internal.
AtMostOneRunning ==
    Cardinality({i \in DOMAIN requests : requests[i].state = "running"}) <= 1

\* Audience: internal. The contract sees the submission and completion doorbells only through DoorbellContract.
SubmissionDoorbellUsed ==
    /\ SubmissionBell!WorkIsTaken
    /\ SubmissionBell!AWaiterIsWoken
CompletionWorkTaken(p) ==
    IF p = 1 THEN CompletionBell1!WorkIsTaken ELSE CompletionBell2!WorkIsTaken
CompletionWakeServed(p) ==
    IF p = 1 THEN CompletionBell1!AWaiterIsWoken ELSE CompletionBell2!AWaiterIsWoken
\* Audience: internal.
CompletionDoorbellsUsed ==
    \A p \in Pairs : CompletionWorkTaken(p) /\ CompletionWakeServed(p)

Witness_AllPairsServed == ~\A p \in Pairs : served[p]
Witness_AQueuedEntryWithdrawn ==
    ~\E i \in DOMAIN requests : requests[i].state = "withdrawn"
Witness_AStartedEntryFinishesAfterDrop ==
    ~\E i \in DOMAIN requests : requests[i].state = "unavailable"
                                /\ requests[i].effect
Witness_CompletionTaken ==
    ~\E p \in Pairs : CompletionTakenAt(p) > 0

=============================================================================
