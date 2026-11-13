@{
    Timeline = 'T27'
    Base = 'SharedPublication-Control-RecoveryLeavesTheDocumentsResolve'
    Note = 'TLC names every step of this spec SpNext, so the actions are the disjuncts of SpNextCore that each step of the trace takes.'
    Steps = @(
        @{ Id = 'ATT'; Actions = @('Quiet(BeginAttach(1))', 'Quiet(TakeGateS(1))', 'Quiet(TakeReaderS(1))', 'Quiet(DropGate(1))', 'Quiet(TakeWriterS(1))', 'SAdmit(1)', 'Quiet(Request(1))', 'Quiet(Lock(1))', 'Quiet(Record(1))', 'Quiet(BeginOpen(1))', 'Quiet(EndOpen(1))') }
        @{ Id = 'ING'; Actions = @('Pick(1, 1)', 'IngRen(1)', 'IngSync(1)') }
        @{ Id = 'PR1'; Actions = @('PtrRen(1)', 'PtrSync(1)') }
        @{ Id = 'PR2'; Actions = @('PtrRen(1)') }
        @{ Id = 'FAI'; Actions = @('SFailUnresolved(1)') }
        @{ Id = 'REC'; Actions = @('Quiet(RecStart(1))', 'Quiet(RecWhole(1))', 'RvBegin(1)', 'SRecEnd(1)', 'Quiet(RecSweep(1))', 'Quiet(RecCount(1))', 'Quiet(RecClear(1))') }
        @{ Id = 'REL'; Actions = @('Quiet(Release(1))', 'Quiet(Unlock(1))') }
    )
}
