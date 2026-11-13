# Copyright (c) Michael Grier.
# Probes of the acquisition layer: each breaks one condition a take waits for, or one rule of
# an upgrader's order, and the property written for it must fail.
# tools/run-probes.ps1 describes the fields and how a mutant stays current.
@{
    Mutants = @(
        @{
            Name      = 'lock-taken-while-held'
            Base      = 'Acquisition-Writers3'
            Kind      = 'INVARIANTS'
            Property  = 'OneWriterAtATime'
            Def       = 'Lock'
            Old = @'
    /\ pc[p] = "waiting" /\ lockHolder = None
'@
            New = @'
    /\ pc[p] = "waiting"
'@
        }
        @{
            Name      = 'gate-shared-past-an-exclusive-holder'
            Base      = 'Acquisition'
            Kind      = 'INVARIANTS'
            Property  = 'GateIsExclusive'
            Def       = 'TakeGateS'
            Old = @'
    /\ att[p] = "wGate" /\ ~GateXHeld
'@
            New = @'
    /\ att[p] = "wGate"
'@
        }
        @{
            Name      = 'gate-exclusive-past-a-shared-holder'
            Base      = 'Acquisition'
            Kind      = 'INVARIANTS'
            Property  = 'GateIsExclusive'
            Def       = 'TakeGateX'
            Old = @'
    /\ att[p] = "uWGate" /\ ~GateSHeld /\ ~GateXHeld
'@
            New = @'
    /\ att[p] = "uWGate" /\ ~GateXHeld
'@
        }
        @{
            Name      = 'reader-tier-exclusive-past-a-reader'
            Base      = 'Acquisition'
            Kind      = 'INVARIANTS'
            Property  = 'TiersAreExclusive'
            Def       = 'TakeFirstTier'
            Old = @'
          THEN /\ ~ReaderSHeld /\ ~ReaderXHeld
'@
            New = @'
          THEN /\ ~ReaderXHeld
'@
        }
        @{
            Name      = 'writer-tier-shared-past-an-upgrader'
            Base      = 'Acquisition-SafeWriter'
            Kind      = 'INVARIANTS'
            Property  = 'TiersAreExclusive'
            Def       = 'TakeWriterS'
            Old = @'
    /\ att[p] = "rd" /\ ~WriterXHeld
'@
            New = @'
    /\ att[p] = "rd"
'@
        }
        @{
            Name      = 'exclusive-first-not-applied'
            Base      = 'Acquisition-ExclusiveFirst'
            Kind      = 'PROPERTIES'
            Property  = 'AWaitingUpgraderIsServed'
            Def       = 'TakeGateS'
            Old = @'
    /\ ~(ExclusiveFirst /\ ExclusiveWaiter)
'@
            New = @'
    /\ TRUE
'@
        }
    )
}
