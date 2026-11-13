@{
    Timeline = 'T6'
    Base = 'Coordination-Control-UpgraderHoldsExclusion'
    Note = 'U is process 1 and W process 2.'
    Steps = @(
        @{ Id = 'BGN1'; Actions = @('BeginAttach(1)') }
        @{ Id = 'GTS1'; Actions = @('TakeGateS(1)') }
        @{ Id = 'RDS1'; Actions = @('TakeReaderS(1)') }
        @{ Id = 'BGN2'; Actions = @('BeginAttach(2)') }
        @{ Id = 'DRP1'; Actions = @('DropGate(1)') }
        @{ Id = 'GTS2'; Actions = @('TakeGateS(2)') }
        @{ Id = 'WRS1'; Actions = @('TakeWriterS(1)') }
        @{ Id = 'ADM1'; Actions = @('Admit(1)') }
        @{ Id = 'REQ1'; Actions = @('Request(1)') }
        @{ Id = 'RDS2'; Actions = @('TakeReaderS(2)') }
        @{ Id = 'LCK1'; Actions = @('Lock(1)') }
        @{ Id = 'REC1'; Actions = @('Record(1)') }
        @{ Id = 'SUPX'; Actions = @('StartUpgradeHolding(1)') }
        @{ Id = 'DRP2'; Actions = @('DropGate(2)') }
        @{ Id = 'WRS2'; Actions = @('TakeWriterS(2)') }
        @{ Id = 'GTX'; Actions = @('TakeGateX(1)') }
        @{ Id = 'ADM2'; Actions = @('Admit(2)') }
        @{ Id = 'REQ2'; Actions = @('Request(2)') }
        @{ Id = 'DLK'; Actions = @() }
    )
}
