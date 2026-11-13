@{
    Timeline = 'T3'
    Base = 'Coordination-Control-ReadUntrustedAsStored'
    Note = 'A crash ends every attachment in the model, so B, which the status line has attached before the timeline begins, attaches after the crash.'
    Before = @(
        'BeginAttach(1)', 'TakeGateS(1)', 'TakeReaderS(1)', 'DropGate(1)', 'TakeWriterS(1)', 'Admit(1)'
    )
    Steps = @(
        @{ Id = 'REQ1'; Actions = @('Request(1)') }
        @{ Id = 'LCK1'; Actions = @('Lock(1)') }
        @{ Id = 'REC1'; Actions = @('Record(1)') }
        @{ Id = 'OPN'; Actions = @('BeginOpen(1)') }
        @{ Id = 'PUB'; Actions = @('EndOpen(1)') }
        @{ Id = 'EFF1'; Actions = @('FirstEffect(1)') }
        @{ Id = 'CRSH'; Actions = @('Crash') }
        @{ Id = 'attach-B'; Omitted = $true; Actions = @('BeginAttach(2)', 'TakeGateS(2)', 'TakeReaderS(2)', 'DropGate(2)', 'TakeWriterS(2)', 'Admit(2)') }
        @{ Id = 'SMP1'; Actions = @('ReadBefore(2)') }
        @{ Id = 'OBSA'; Actions = @('ObserveA(2)') }
        @{ Id = 'OBSB'; Actions = @('ObserveB(2)') }
        @{ Id = 'SMP2'; Actions = @('ReadAfter(2)') }
    )
}
