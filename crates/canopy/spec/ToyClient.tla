\* Copyright (c) Michael Grier.
----------------------------- MODULE ToyClient -----------------------------
(***************************************************************************
 * A dependent that imports only the public client contract. Its local state
 * replaces the contract's contents parameter, while every request and
 * session variable remains owned by the contract instance.
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

VARIABLES
    sessionState,
    pairState,
    submissions,
    applicationContents,
    recoveryRequired

Canopy == INSTANCE ClientContract WITH contents <- applicationContents

Init == Canopy!Init
Next == Canopy!Next
Spec == Canopy!ClientLiveSpec

\* Audience: client. The dependent sees logical value through the contract projection. Shows through: application state.
ApplicationContentsArePublic == applicationContents \in Values

\* Audience: client. The dependent uses only public completion correlation. Shows through: command completion.
ApplicationCannotDuplicateACompletion == Canopy!AtMostOneDelivery

=============================================================================
