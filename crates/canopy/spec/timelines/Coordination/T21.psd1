@{
    Timeline = 'T21'
    Base = 'Coordination-Control-UpgradeIgnoresRecoveryRequired'
    Note = 'A is process 1. PrepareFails is the failure that leaves no record. The upgrade body takes writer exclusion as it begins.'
    Before = @(
        'BeginAttach(1)', 'TakeGateS(1)', 'TakeReaderS(1)', 'DropGate(1)', 'TakeWriterS(1)', 'Admit(1)'
    )
    Steps = @(
        @{ Id = 'REQ1'; Actions = @('Request(1)') }
        @{ Id = 'LCK1'; Actions = @('Lock(1)') }
        @{ Id = 'REC1'; Actions = @('Record(1)') }
        @{ Id = 'OPN1'; Actions = @('BeginOpen(1)') }
        @{ Id = 'PUB1'; Actions = @('EndOpen(1)') }
        @{ Id = 'BEGN'; Actions = @('Begin(1)') }
        @{ Id = 'STG'; Actions = @('Ingest(1)') }
        @{ Id = 'PRPF'; Actions = @('PrepareFails(1)') }
        @{ Id = 'CLR'; Actions = @('Release(1)') }
        @{ Id = 'ULK'; Actions = @('Unlock(1)') }
        @{ Id = 'UPST'; Actions = @('StartUpgrade(1)') }
        @{ Id = 'DRN'; Actions = @('DrainDone(1)') }
        @{ Id = 'GTX'; Actions = @('TakeGateX(1)') }
        @{ Id = 'TRS'; Actions = @('TakeFirstTier(1)') }
        @{ Id = 'LCK2'; Actions = @('BeginUpgrade(1)') }
        @{ Id = 'REC2'; Actions = @('Record(1)') }
    )
}
