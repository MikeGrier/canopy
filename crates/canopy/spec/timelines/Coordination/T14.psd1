@{
    Timeline = 'T14'
    Base = 'Coordination-Control-CountPublishedBeforeTheNamespaceIsWholeTornRead'
    Note = 'The control with a read. PrepareFails is the failure whose record reaches the log.'
    Before = @(
        'BeginAttach(1)', 'TakeGateS(1)', 'TakeReaderS(1)', 'DropGate(1)', 'TakeWriterS(1)', 'Admit(1)'
    )
    Steps = @(
        @{ Id = 'REQ1'; Actions = @('Request(1)') }
        @{ Id = 'LCK1'; Actions = @('Lock(1)') }
        @{ Id = 'REC1'; Actions = @('Record(1)') }
        @{ Id = 'OPN'; Actions = @('BeginOpen(1)') }
        @{ Id = 'PUB'; Actions = @('EndOpen(1)') }
        @{ Id = 'BEGN'; Actions = @('Begin(1)') }
        @{ Id = 'STG'; Actions = @('Ingest(1)') }
        @{ Id = 'PRPF'; Actions = @('PrepareFails(1)') }
        @{ Id = 'RSRT'; Actions = @('RecStart(1)') }
        @{ Id = 'RCNT'; Actions = @('RecCount(1)') }
        @{ Id = 'RSWA'; Actions = @('RecSwap1(1)') }
        @{ Id = 'RSTP'; Actions = @('RecStop(1)') }
        @{ Id = 'CLR'; Actions = @('Release(1)') }
        @{ Id = 'ULK'; Actions = @('Unlock(1)') }
        @{ Id = 'SMP1'; Actions = @('ReadBefore(1)') }
        @{ Id = 'OBSA'; Actions = @('ObserveA(1)') }
        @{ Id = 'OBSB'; Actions = @('ObserveB(1)') }
        @{ Id = 'SMP2'; Actions = @('ReadAfter(1)') }
    )
}
