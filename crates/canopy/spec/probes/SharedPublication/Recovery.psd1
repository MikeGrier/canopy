# Copyright (c) Michael Grier.
# Probes of recovery's work on the documents in the composition.
# tools/run-probes.ps1 describes the fields and how a mutant stays current.
@{
    Mutants = @(
        @{
            Name      = 'record-recovery-skips-the-record-sync'
            Base      = 'SharedPublication'
            Kind      = 'INVARIANTS'
            Property  = 'OutcomeIsFixedOnceRecovered'
            Def       = 'RvRecSync'
            Old = @'
    /\ record' = [record EXCEPT ![wdoc].dur = record[wdoc].live]
'@
            New = @'
    /\ record' = record
'@
        }
    )
}
