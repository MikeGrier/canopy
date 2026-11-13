@{
    Timeline = 'T17'
    Base = 'Coordination-Control-DiscoveryTrustsTheOwnershipRecord'
    Note = 'A is process 1 and B process 2.'
    Before = @(
        'BeginAttach(1)', 'TakeGateS(1)', 'TakeReaderS(1)', 'DropGate(1)', 'TakeWriterS(1)', 'Admit(1)', 'BeginAttach(2)', 'TakeGateS(2)', 'TakeReaderS(2)', 'DropGate(2)', 'TakeWriterS(2)', 'Admit(2)'
    )
    Steps = @(
        @{ Id = 'REQ1'; Actions = @('Request(1)') }
        @{ Id = 'LCK1'; Actions = @('Lock(1)') }
        @{ Id = 'REC1'; Actions = @('Record(1)') }
        @{ Id = 'DISC'; Actions = @('Discover(2)') }
    )
}
