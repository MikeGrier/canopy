\* Copyright (c) Michael Grier.
------------------------------ MODULE RingCheck ------------------------------
(***************************************************************************)
(* Negative controls for Ring. Each mutation stays outside the mechanism   *)
(* so Ring's public actions and composed contracts remain control-free.     *)
(***************************************************************************)
EXTENDS Ring

CONSTANT Control
ASSUME Control \in {"overfill", "cancel-started", "skip-ready", "lost-wake"}

FaultOverfill(p) ==
    /\ Control = "overfill"
    /\ p \in Pairs
    /\ live[p]
    /\ Outstanding(p) >= CompletionCapacity
    /\ Len(requests) < MaxRequests
    /\ PushAccepted(p)

FaultCancelStarted ==
    /\ Control = "cancel-started"
    /\ running # NoRequest
    /\ LET i == running
           p == requests[i].pair
       IN /\ live' = [live EXCEPT ![p] = FALSE]
          /\ announced' = [announced EXCEPT ![p] = FALSE]
          /\ requests' = [j \in DOMAIN requests |->
                IF requests[j].pair # p THEN requests[j]
                ELSE IF requests[j].state = "queued" \/ j = i
                THEN [requests[j] EXCEPT !.state = "withdrawn"]
                ELSE requests[j]]
          /\ contractRequests' = [j \in DOMAIN contractRequests |->
                IF contractRequests[j].pair = p
                   /\ contractRequests[j].state \in {"accepted", "complete"}
                THEN [contractRequests[j] EXCEPT !.state = "unavailable"]
                ELSE contractRequests[j]]
          /\ running' = NoRequest
          /\ UNCHANGED <<nextRequest, cursor, served, skipped,
                         submissionPublished, submissionTaken, notifications,
                         submissionConsumer>>
          /\ UNCHANGED completionVars

FaultStartOutOfOrder(p) ==
    /\ Control = "skip-ready"
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
    /\ skipped' = {q \in Pairs : Ready(q)
                              /\ Distance(cursor, q) < Distance(cursor, p)}
    /\ UNCHANGED <<live, nextRequest, contractRequests, submissionPublished,
                   submissionTaken, notifications, submissionConsumer>>
    /\ UNCHANGED completionVars

FaultLostSubmissionWake(p) ==
    /\ Control = "lost-wake"
    /\ p \in Pairs
    /\ live[p]
    /\ ~announced[p]
    /\ \E i \in DOMAIN requests : requests[i].pair = p
                                /\ requests[i].state = "queued"
    /\ announced' = [announced EXCEPT ![p] = TRUE]
    /\ notifications' = notifications + 1
    /\ UNCHANGED <<live, nextRequest, requests, contractRequests, running,
                   cursor, served, skipped, submissionPublished, submissionTaken,
                   submissionConsumer>>
    /\ UNCHANGED completionVars

FaultNext ==
    \/ Next
    \/ \E p \in Pairs : FaultOverfill(p)
    \/ FaultCancelStarted
    \/ \E p \in Pairs : FaultStartOutOfOrder(p)
    \/ \E p \in Pairs : FaultLostSubmissionWake(p)

CheckSpec == Init /\ [][FaultNext]_vars

=============================================================================
