@{
    Timeline = 'T2'
    Base = 'Coordination-Control-UnlockBeforeClear'
    Note = 'In the control, Release drops the lock and keeps the record.'
    Before = @(
        'BeginAttach(1)', 'TakeGateS(1)', 'TakeReaderS(1)', 'DropGate(1)', 'TakeWriterS(1)', 'Admit(1)', 'BeginAttach(2)', 'TakeGateS(2)', 'TakeReaderS(2)', 'DropGate(2)', 'TakeWriterS(2)', 'Admit(2)'
    )
    Steps = @(
        @{ Id = 'REQ1'; Actions = @('Request(1)') }
        @{ Id = 'LCK1'; Actions = @('Lock(1)') }
        @{ Id = 'REC1'; Actions = @('Record(1)') }
        @{ Id = 'ULK'; Actions = @('Release(1)') }
        @{ Id = 'REQ2'; Actions = @('Request(2)') }
        @{ Id = 'LCK2'; Actions = @('Lock(2)') }
        @{ Id = 'FNC'; Actions = @('SetFence(2)') }
        @{ Id = 'REC2'; Actions = @('Record(2)') }
    )
}
