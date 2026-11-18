@{
    Timeline = 'T32'
    Base = 'Creation-TwoCreators'
    Overrides = @{ NArt = '2'; MaxCrashes = '0'; Intrusion = 'FALSE' }
    Note = 'A is process 1 and B process 2, creators of one implementation with the same inputs. The marker is the last artifact.'
    Steps = @(
        @{ Id = 'LOOK1'; Actions = @('Look(1)') }
        @{ Id = 'LOOK2'; Actions = @('Look(2)') }
        @{ Id = 'LOCK1'; Actions = @('Lock(2)') }
        @{ Id = 'PUB'; Actions = @('Publish(2)') }
        @{ Id = 'SYNR'; Actions = @('SyncRes(2)') }
        @{ Id = 'KILL'; Actions = @('Terminate(2)') }
        @{ Id = 'LOCK2'; Actions = @('Lock(1)') }
        @{ Id = 'TAKE'; Actions = @('TakeOver(1)') }
        @{ Id = 'SYNR2'; Actions = @('SyncRes(1)') }
        @{ Id = 'WRA1'; Actions = @('WriteArt(1)') }
        @{ Id = 'SYA1'; Actions = @('SyncArt(1)') }
        @{ Id = 'WRM'; Actions = @('WriteArt(1)') }
        @{ Id = 'SYM'; Actions = @('SyncArt(1)') }
    )
}
