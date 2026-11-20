@{
    Timeline = 'T19'
    Base = 'Coordination-Control-StepBeforeTheIntent'
    Note = 'A is process 1. The migration intent and step are Store client work outside the coordination contract, so the coordination bridge follows the timeline.'
    Departs = $null
    Before = @(
        'BeginAttach(1)', 'TakeGateS(1)', 'TakeReaderS(1)', 'DropGate(1)', 'TakeWriterS(1)', 'Admit(1)', 'StartUpgrade(1)', 'DrainDone(1)', 'TakeGateX(1)', 'TakeFirstTier(1)'
    )
    Steps = @(
        @{ Id = 'LCK1'; Actions = @('BeginUpgrade(1)') }
        @{ Id = 'REC1'; Actions = @('Record(1)') }
        @{ Id = 'OPN'; Actions = @('BeginOpen(1)') }
        @{ Id = 'PUB'; Actions = @('EndOpen(1)') }
        @{ Id = 'STEP1'; Actions = @('UpStep(1)') }
    )
}
