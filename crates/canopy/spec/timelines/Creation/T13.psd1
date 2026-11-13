@{
    Timeline = 'T13'
    Base = 'Creation-Control-OpenTrustsALiveMarker'
    Overrides = @{ MaxCrashes = '1' }
    Note = 'A is process 1 and O process 2. Step 10 needs a crash, which the control''s constants do not allow.'
    Steps = @(
        @{ Id = 'LOOK'; Actions = @('Look(1)') }
        @{ Id = 'LOCK'; Actions = @('Lock(1)') }
        @{ Id = 'PUB'; Actions = @('Publish(1)') }
        @{ Id = 'SYNR'; Actions = @('SyncRes(1)') }
        @{ Id = 'WRA1'; Actions = @('WriteArt(1)') }
        @{ Id = 'SYA1'; Actions = @('SyncArt(1)') }
        @{ Id = 'WRM'; Actions = @('WriteArt(1)') }
        @{ Id = 'KILL'; Actions = @('Die(1)') }
        @{ Id = 'OPEN'; Actions = @('OpenLook(2)') }
        @{ Id = 'CRSH'; Actions = @('Crash') }
    )
}
