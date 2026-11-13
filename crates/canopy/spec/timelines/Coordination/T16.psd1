@{
    Timeline = 'T16'
    Base = 'Coordination-Witness-ARecoveryIsResumedAfterADeathBetweenItsSwaps'
    Note = 'A is process 1 and B process 2.'
    Before = @(
        'BeginAttach(1)', 'TakeGateS(1)', 'TakeReaderS(1)', 'DropGate(1)', 'TakeWriterS(1)', 'Admit(1)', 'BeginAttach(2)', 'TakeGateS(2)', 'TakeReaderS(2)', 'DropGate(2)', 'TakeWriterS(2)', 'Admit(2)'
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
        @{ Id = 'RSRT1'; Actions = @('RecStart(1)') }
        @{ Id = 'RSWA1'; Actions = @('RecSwap1(1)') }
        @{ Id = 'KILL'; Actions = @('Die(1)') }
        @{ Id = 'REQ2'; Actions = @('Request(2)') }
        @{ Id = 'LCK2'; Actions = @('Lock(2)') }
        @{ Id = 'REC2'; Actions = @('Record(2)') }
        @{ Id = 'RSRT2'; Actions = @('RecStart(2)') }
        @{ Id = 'RSWA2'; Actions = @('RecSwap1(2)') }
        @{ Id = 'RSWB'; Actions = @('RecSwap2(2)') }
        @{ Id = 'REND'; Actions = @('RecEnd(2)') }
        @{ Id = 'RSWP'; Actions = @('RecSweep(2)') }
        @{ Id = 'RCNT'; Actions = @('RecCount(2)') }
        @{ Id = 'RCLR'; Actions = @('RecClear(2)') }
    )
}
