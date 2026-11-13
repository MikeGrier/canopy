@{
    Timeline = 'T4'
    Base = 'Coordination-Control-UnsafeWriterTierFirst'
    Note = 'U is process 1 and W process 2. Starting the upgrade is two actions: the drain, and its end, which lets go of the tiers.'
    Steps = @(
        @{ Id = 'BGN1'; Actions = @('BeginAttach(2)') }
        @{ Id = 'GTS1'; Actions = @('TakeGateS(2)') }
        @{ Id = 'BGN2'; Actions = @('BeginAttach(1)') }
        @{ Id = 'GTS2'; Actions = @('TakeGateS(1)') }
        @{ Id = 'RDS1'; Actions = @('TakeReaderS(1)') }
        @{ Id = 'DRP1'; Actions = @('DropGate(1)') }
        @{ Id = 'WRS1'; Actions = @('TakeWriterS(1)') }
        @{ Id = 'ADM1'; Actions = @('Admit(1)') }
        @{ Id = 'SUP'; Actions = @('StartUpgrade(1)', 'DrainDone(1)') }
        @{ Id = 'RDS2'; Actions = @('TakeReaderS(2)') }
        @{ Id = 'DRP2'; Actions = @('DropGate(2)') }
        @{ Id = 'GTX'; Actions = @('TakeGateX(1)') }
        @{ Id = 'WRX'; Actions = @('TakeFirstTier(1)') }
        @{ Id = 'DLK'; Actions = @() }
    )
}
