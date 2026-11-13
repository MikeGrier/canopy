@{
    Timeline = 'T15'
    Base = 'Coordination-Control-FenceClearedAtRecoveryStart'
    Overrides = @{ MaxRequests = '2' }
    Note = 'A requests twice, which the control''s constants do not allow. PrepareFails is the failure that leaves no record.'
    Before = @(
        'BeginAttach(1)', 'TakeGateS(1)', 'TakeReaderS(1)', 'DropGate(1)', 'TakeWriterS(1)', 'Admit(1)'
    )
    Steps = @(
        @{ Id = 'REQ1'; Actions = @('Request(1)') }
        @{ Id = 'LCK1'; Actions = @('Lock(1)') }
        @{ Id = 'REC1'; Actions = @('Record(1)') }
        @{ Id = 'OPN1'; Actions = @('BeginOpen(1)') }
        @{ Id = 'PUB1'; Actions = @('EndOpen(1)') }
        @{ Id = 'BEGN1'; Actions = @('Begin(1)') }
        @{ Id = 'STG'; Actions = @('Ingest(1)') }
        @{ Id = 'PRPF'; Actions = @('PrepareFails(1)') }
        @{ Id = 'RSRT'; Actions = @('RecStart(1)') }
        @{ Id = 'RSTP'; Actions = @('RecStop(1)') }
        @{ Id = 'CLR'; Actions = @('Release(1)') }
        @{ Id = 'ULK'; Actions = @('Unlock(1)') }
        @{ Id = 'REQ2'; Actions = @('Request(1)') }
        @{ Id = 'LCK2'; Actions = @('Lock(1)') }
        @{ Id = 'REC2'; Actions = @('Record(1)') }
        @{ Id = 'OPN2'; Actions = @('BeginOpen(1)') }
        @{ Id = 'PUB2'; Actions = @('EndOpen(1)') }
        @{ Id = 'BEGN2'; Actions = @('Begin(1)') }
    )
}
