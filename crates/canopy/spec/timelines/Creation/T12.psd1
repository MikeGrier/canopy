@{
    Timeline = 'T12'
    Base = 'Creation-Control-ResumeWithoutReread'
    Note = 'A is process 1 and B process 2. The marker is the last artifact.'
    Steps = @(
        @{ Id = 'LOOK1'; Actions = @('Look(1)') }
        @{ Id = 'LOCK1'; Actions = @('Lock(1)') }
        @{ Id = 'PUB'; Actions = @('Publish(1)') }
        @{ Id = 'SYNR'; Actions = @('SyncRes(1)') }
        @{ Id = 'LOOK2'; Actions = @('Look(2)') }
        @{ Id = 'WRA1'; Actions = @('WriteArt(1)') }
        @{ Id = 'SYA1'; Actions = @('SyncArt(1)') }
        @{ Id = 'WRM'; Actions = @('WriteArt(1)') }
        @{ Id = 'SYM'; Actions = @('SyncArt(1)') }
        @{ Id = 'LOCK2'; Actions = @('Lock(2)') }
        @{ Id = 'CNFM'; Actions = @('Reconfirm(2)') }
    )
}
