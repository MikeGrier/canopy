@{
    Timeline = 'T5'
    Base = 'Coordination-Control-UpgraderKeepsTiers'
    Note = 'U1 is process 1 and U2 process 2.'
    Steps = @(
        @{ Id = 'BGN1'; Actions = @('BeginAttach(1)') }
        @{ Id = 'GTS1'; Actions = @('TakeGateS(1)') }
        @{ Id = 'RDS1'; Actions = @('TakeReaderS(1)') }
        @{ Id = 'BGN2'; Actions = @('BeginAttach(2)') }
        @{ Id = 'GTS2'; Actions = @('TakeGateS(2)') }
        @{ Id = 'RDS2'; Actions = @('TakeReaderS(2)') }
        @{ Id = 'DRP1'; Actions = @('DropGate(1)') }
        @{ Id = 'WRS1'; Actions = @('TakeWriterS(1)') }
        @{ Id = 'ADM1'; Actions = @('Admit(1)') }
        @{ Id = 'SUP1'; Actions = @('StartUpgrade(1)', 'DrainDone(1)') }
        @{ Id = 'DRP2'; Actions = @('DropGate(2)') }
        @{ Id = 'WRS2'; Actions = @('TakeWriterS(2)') }
        @{ Id = 'GTX1'; Actions = @('TakeGateX(1)') }
        @{ Id = 'ADM2'; Actions = @('Admit(2)') }
        @{ Id = 'SUP2'; Actions = @('StartUpgrade(2)', 'DrainDone(2)') }
        @{ Id = 'DLK'; Actions = @() }
    )
}
