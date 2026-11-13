@{
    Timeline = 'T22'
    Base = 'Coordination-Control-UnsafeJudgedAgainstTheCurrentSchema'
    Note = 'A is process 1, B process 2 and C, the reader, process 3. The replay runs in the model itself; the scratch copy the timeline names only removed behaviours.'
    Steps = @(
        @{ Id = 'ATT1'; Actions = @('BeginAttach(3)') }
        @{ Id = 'GATE1'; Actions = @('TakeGateS(3)') }
        @{ Id = 'RTIR1'; Actions = @('TakeReaderS(3)') }
        @{ Id = 'DROP1'; Actions = @('DropGate(3)') }
        @{ Id = 'ADMT1'; Actions = @('Admit(3)') }
        @{ Id = 'ATT2'; Actions = @('BeginAttach(1)') }
        @{ Id = 'GATE2'; Actions = @('TakeGateS(1)') }
        @{ Id = 'RTIR2'; Actions = @('TakeReaderS(1)') }
        @{ Id = 'DROP2'; Actions = @('DropGate(1)') }
        @{ Id = 'WTIR1'; Actions = @('TakeWriterS(1)') }
        @{ Id = 'ADMT2'; Actions = @('Admit(1)') }
        @{ Id = 'UPST1'; Actions = @('StartUpgrade(1)') }
        @{ Id = 'DRN1'; Actions = @('DrainDone(1)') }
        @{ Id = 'GTX1'; Actions = @('TakeGateX(1)') }
        @{ Id = 'TRS1'; Actions = @('TakeFirstTier(1)') }
        @{ Id = 'BGN1'; Actions = @('BeginUpgrade(1)') }
        @{ Id = 'END1'; Actions = @('EndUpgrade(1)') }
        @{ Id = 'ATT3'; Actions = @('BeginAttach(2)') }
        @{ Id = 'GATE3'; Actions = @('TakeGateS(2)') }
        @{ Id = 'RTIR3'; Actions = @('TakeReaderS(2)') }
        @{ Id = 'DROP3'; Actions = @('DropGate(2)') }
        @{ Id = 'WTIR2'; Actions = @('TakeWriterS(2)') }
        @{ Id = 'ADMT3'; Actions = @('Admit(2)') }
        @{ Id = 'UPST2'; Actions = @('StartUpgrade(2)') }
        @{ Id = 'DRN2'; Actions = @('DrainDone(2)') }
        @{ Id = 'GTX2'; Actions = @('TakeGateX(2)') }
        @{ Id = 'TRS2'; Actions = @('TakeFirstTier(2)') }
        @{ Id = 'BGN2'; Actions = @('BeginUpgrade(2)') }
        @{ Id = 'END2'; Actions = @('EndUpgrade(2)') }
    )
}
