@{
    Timeline = 'T9'
    Base = 'Coordination-Control-ListWithoutClaim'
    Note = 'A is process 1; B and its listing thread L are process 2.'
    Steps = @(
        @{ Id = 'BGN1'; Actions = @('BeginAttach(1)') }
        @{ Id = 'GTS1'; Actions = @('TakeGateS(1)') }
        @{ Id = 'RDS1'; Actions = @('TakeReaderS(1)') }
        @{ Id = 'DRP1'; Actions = @('DropGate(1)') }
        @{ Id = 'WRS1'; Actions = @('TakeWriterS(1)') }
        @{ Id = 'ADM1'; Actions = @('Admit(1)') }
        @{ Id = 'RGS1'; Actions = @('RegStart(1)') }
        @{ Id = 'RGC1'; Actions = @('RegClaim(1, 1)') }
        @{ Id = 'RGL1'; Actions = @('RegLive(1)') }
        @{ Id = 'RGW1'; Actions = @('RegWrite(1)') }
        @{ Id = 'KILL'; Actions = @('Die(1)') }
        @{ Id = 'BGN2'; Actions = @('BeginAttach(2)') }
        @{ Id = 'GTS2'; Actions = @('TakeGateS(2)') }
        @{ Id = 'RDS2'; Actions = @('TakeReaderS(2)') }
        @{ Id = 'DRP2'; Actions = @('DropGate(2)') }
        @{ Id = 'WRS2'; Actions = @('TakeWriterS(2)') }
        @{ Id = 'ADM2'; Actions = @('Admit(2)') }
        @{ Id = 'RGS2'; Actions = @('RegStart(2)') }
        @{ Id = 'RGC2'; Actions = @('RegClaim(2, 1)') }
        @{ Id = 'RGL2'; Actions = @('RegLive(2)') }
        @{ Id = 'LSB'; Actions = @('ListStart(2)') }
        @{ Id = 'LCL'; Actions = @('ListClaim(2)') }
        @{ Id = 'LRD'; Actions = @('ListRead(2)') }
        @{ Id = 'LPB'; Actions = @('ListProbe(2)') }
    )
}
