@{
    Timeline = 'T24'
    Base = 'PointerPublication-Control-SettleWithoutSyncStrandsTheVote'
    Note = 'Ingesting a blob renames and syncs it, then syncs its directory.'
    Steps = @(
        @{ Id = 'ING'; Actions = @('StartWrite("B")', 'IngestRename', 'IngestSync', 'IngestRename', 'IngestSync') }
        @{ Id = 'REC'; Actions = @('RecWrite') }
        @{ Id = 'RSF'; Actions = @('Fail') }
        @{ Id = 'OPN'; Actions = @('Quiesce', 'Close', 'Open') }
        @{ Id = 'RFA'; Actions = @('RfRename', 'RfSync') }
        @{ Id = 'RFB'; Actions = @('RfRename', 'RfSync') }
        @{ Id = 'PWR'; Actions = @('Crash') }
        @{ Id = 'OP2'; Actions = @('Open') }
        @{ Id = 'WR2'; Actions = @('StartWrite("C")', 'IngestRename', 'IngestSync', 'IngestRename', 'IngestSync', 'RecWrite', 'RecSync') }
        @{ Id = 'PRN'; Actions = @('PtrRename') }
        @{ Id = 'FAI'; Actions = @('Fail') }
    )
}
