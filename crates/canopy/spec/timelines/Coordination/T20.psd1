@{
    Timeline = 'T20'
    Base = 'Coordination-Control-InterruptedUpgradeAdmitsSourceWriters'
    Note = 'A is process 1 and B process 2. The upgrade body takes writer exclusion as it begins.'
    Before = @(
        'BeginAttach(1)', 'TakeGateS(1)', 'TakeReaderS(1)', 'DropGate(1)', 'TakeWriterS(1)', 'Admit(1)', 'StartUpgrade(1)', 'DrainDone(1)', 'TakeGateX(1)', 'TakeFirstTier(1)'
    )
    Steps = @(
        @{ Id = 'LCK1'; Actions = @('BeginUpgrade(1)') }
        @{ Id = 'REC1'; Actions = @('Record(1)') }
        @{ Id = 'OPN'; Actions = @('BeginOpen(1)') }
        @{ Id = 'PUB'; Actions = @('EndOpen(1)') }
        @{ Id = 'INTN'; Actions = @('RecordIntent(1)') }
        @{ Id = 'FAIL'; Actions = @('UpFails(1)') }
        @{ Id = 'KILL'; Actions = @('Die(1)') }
        @{ Id = 'ATT'; Actions = @('BeginAttach(2)') }
        @{ Id = 'GATE'; Actions = @('TakeGateS(2)') }
        @{ Id = 'RTIR'; Actions = @('TakeReaderS(2)') }
        @{ Id = 'DROP'; Actions = @('DropGate(2)') }
        @{ Id = 'WTIR'; Actions = @('TakeWriterS(2)') }
        @{ Id = 'ADMT'; Actions = @('Admit(2)') }
        @{ Id = 'REQ2'; Actions = @('Request(2)') }
        @{ Id = 'LCK2'; Actions = @('Lock(2)') }
        @{ Id = 'REC2'; Actions = @('Record(2)') }
        @{ Id = 'RSRT'; Actions = @('RecStart(2)') }
    )
}
