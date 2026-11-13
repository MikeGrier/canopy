@{
    Timeline = 'T8'
    Base = 'Coordination-Control-ListingInfersFromFullness'
    Note = 'H and its listing thread L are both process 1.'
    Steps = @(
        @{ Id = 'BGN1'; Actions = @('BeginAttach(1)') }
        @{ Id = 'GTS1'; Actions = @('TakeGateS(1)') }
        @{ Id = 'RDS1'; Actions = @('TakeReaderS(1)') }
        @{ Id = 'DRP1'; Actions = @('DropGate(1)') }
        @{ Id = 'WRS1'; Actions = @('TakeWriterS(1)') }
        @{ Id = 'ADM1'; Actions = @('Admit(1)') }
        @{ Id = 'RGS1'; Actions = @('RegStart(1)') }
        @{ Id = 'LSB'; Actions = @('ListStart(1)') }
        @{ Id = 'LCL'; Actions = @('ListClaim(1)') }
        @{ Id = 'RGC1'; Actions = @('RegClaim(1, 1)') }
        @{ Id = 'OVF'; Actions = @('RegOverflow(1)') }
        @{ Id = 'LRD'; Actions = @('ListRead(1)') }
        @{ Id = 'LPB'; Actions = @('ListProbe(1)') }
        @{ Id = 'LDN'; Actions = @('ListDone(1)') }
    )
}
