\* Copyright (c) Michael Grier.
-------------------------- MODULE RingContract --------------------------
(***************************************************************************)
(* The observable contract of one in-process ring engine. A caller may      *)
(* submit a request into a live queue pair, receive a local request id,     *)
(* take its completion, or lose its completion when the pair is dropped.   *)
(* Queue storage, doorbells, the engine, and the started state are hidden.  *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, Sequences

CONSTANTS
    Pairs,
    CompletionCapacity,
    MaxRequests

ASSUME Pairs # {}
ASSUME CompletionCapacity \in Nat \ {0}
ASSUME MaxRequests \in Nat

PairClosed == "PairClosed"
CompletionQueueFull == "CompletionQueueFull"
None == "none"

VARIABLES
    live,
    nextRequest,
    requests

vars == <<live, nextRequest, requests>>

Init ==
    /\ live = [p \in Pairs |-> TRUE]
    /\ nextRequest = [p \in Pairs |-> 1]
    /\ requests = <<>>

Outstanding(p) ==
    Cardinality({i \in DOMAIN requests :
        requests[i].pair = p /\ requests[i].state \in {"accepted", "complete"}})

Accepted(p) ==
    /\ Len(requests) < MaxRequests
    /\ requests' = Append(requests,
        [pair |-> p, command |-> "command", request |-> nextRequest[p], state |-> "accepted",
         refusal |-> None])
    /\ nextRequest' = [nextRequest EXCEPT ![p] = @ + 1]
    /\ UNCHANGED live

Refused(p, reason) ==
    /\ Len(requests) < MaxRequests
    /\ requests' = Append(requests,
        [pair |-> p, command |-> "command", request |-> 0,
         state |-> "refused", refusal |-> reason])
    /\ UNCHANGED <<live, nextRequest>>

Submit(p) ==
    /\ p \in Pairs
    /\ IF ~live[p]
       THEN Refused(p, PairClosed)
       ELSE IF Outstanding(p) >= CompletionCapacity
       THEN Refused(p, CompletionQueueFull)
       ELSE Accepted(p)

IsNext(p, request) ==
    \A i \in DOMAIN requests :
        requests[i].pair = p /\ requests[i].state = "accepted"
            => request <= requests[i].request

Complete(i) ==
    /\ i \in DOMAIN requests
    /\ requests[i].state = "accepted"
    /\ live[requests[i].pair]
    /\ IsNext(requests[i].pair, requests[i].request)
    /\ requests' = [requests EXCEPT ![i] =
        [requests[i] EXCEPT !.state = "complete"]]
    /\ UNCHANGED <<live, nextRequest>>

CompleteAny == \E i \in DOMAIN requests : Complete(i)

TakeCompletion(i) ==
    /\ i \in DOMAIN requests
    /\ requests[i].state = "complete"
    /\ live[requests[i].pair]
    /\ requests' = [requests EXCEPT ![i] =
        [requests[i] EXCEPT !.state = "delivered"]]
    /\ UNCHANGED <<live, nextRequest>>

DropPair(p) ==
    /\ p \in Pairs
    /\ live[p]
    /\ live' = [live EXCEPT ![p] = FALSE]
    /\ requests' = [i \in DOMAIN requests |->
        IF requests[i].pair = p
           /\ requests[i].state \in {"accepted", "complete"}
        THEN [requests[i] EXCEPT !.state = "unavailable"]
        ELSE requests[i]]
    /\ UNCHANGED nextRequest

Next ==
    \/ \E p \in Pairs : Submit(p)
    \/ CompleteAny
    \/ \E i \in DOMAIN requests : TakeCompletion(i)
    \/ \E p \in Pairs : DropPair(p)

Spec == Init /\ [][Next]_vars

\* Audience: internal.
TypeOK ==
    /\ live \in [Pairs -> BOOLEAN]
    /\ nextRequest \in [Pairs -> Nat]
    /\ requests \in Seq([pair : Pairs,
                         command : {"command"},
                         request : Nat,
                         state : {"accepted", "complete", "delivered",
                                  "unavailable", "refused"},
                         refusal : {PairClosed, CompletionQueueFull, None}])

\* Audience: client. A live pair accepts only within reserved completion capacity. Shows through: accepted or refused submission.
CompletionCapacityReserved ==
    \A p \in Pairs : Outstanding(p) <= CompletionCapacity

\* Audience: client. A request identity appears at most once. Shows through: completion correlation.
UniqueRequestIdentity ==
    \A i, j \in DOMAIN requests :
        /\ i # j
        /\ requests[i].state # "refused"
        /\ requests[j].state # "refused"
        /\ requests[i].pair = requests[j].pair
        => requests[i].request # requests[j].request

\* Audience: client. A later request cannot complete before an earlier accepted request. Shows through: completion order.
PerPairCompletionOrder ==
    \A i, j \in DOMAIN requests :
        /\ requests[i].pair = requests[j].pair
        /\ requests[i].request < requests[j].request
        /\ requests[j].state \in {"complete", "delivered"}
        => requests[i].state # "accepted"

\* Audience: client. An undelivered completion belongs to a live pair. Shows through: completion availability.
UndeliveredCompletionHasLivePair ==
    \A i \in DOMAIN requests :
        requests[i].state = "complete" => live[requests[i].pair]

\* Audience: client. Refusal has no request identity. Shows through: submission outcome.
RefusalHasNoIdentity ==
    \A i \in DOMAIN requests :
        requests[i].state = "refused" <=> requests[i].request = 0

Witness_AnAcceptedCompletion ==
    ~\E i \in DOMAIN requests : requests[i].state \in {"complete", "delivered"}

Witness_AClosedPairRefusal ==
    ~\E i \in DOMAIN requests : requests[i].refusal = PairClosed

Witness_AFullCompletionRefusal ==
    ~\E i \in DOMAIN requests : requests[i].refusal = CompletionQueueFull

Witness_PairLoss ==
    ~\E i \in DOMAIN requests : requests[i].state = "unavailable"

=============================================================================
