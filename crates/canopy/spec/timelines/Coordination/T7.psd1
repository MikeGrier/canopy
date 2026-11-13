@{
    Timeline = 'T7'
    Base = 'Coordination-Control-UpgradeFromInsideAnOperation'
    Note = 'U is process 1. The other process takes no part, as the timeline says, and the replay needs none of its steps.'
    Steps = @(
        @{ Id = 'BGN1'; Actions = @('BeginAttach(1)') }
        @{ Id = 'GTS1'; Actions = @('TakeGateS(1)') }
        @{ Id = 'RDS1'; Actions = @('TakeReaderS(1)') }
        @{ Id = 'DRP1'; Actions = @('DropGate(1)') }
        @{ Id = 'WRS1'; Actions = @('TakeWriterS(1)') }
        @{ Id = 'ADM1'; Actions = @('Admit(1)') }
        @{ Id = 'RDB'; Actions = @('ReadBefore(1)') }
        @{ Id = 'SUPI'; Actions = @('StartUpgrade(1)') }
        @{ Id = 'DLK'; Actions = @() }
    )
}
