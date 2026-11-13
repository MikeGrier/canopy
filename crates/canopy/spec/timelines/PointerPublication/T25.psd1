@{
    Timeline = 'T25'
    Base = 'PointerPublication-Control-SettleLooksOnlyAtTheFile-LivePointerHasBlob'
    Note = 'Ingesting a blob renames and syncs it, then syncs its directory.'
    Steps = @(
        @{ Id = 'WR1'; Actions = @('StartWrite("A")', 'IngestRename', 'IngestSync', 'IngestRename', 'IngestSync', 'RecWrite', 'RecSync') }
        @{ Id = 'SW1'; Actions = @('PtrRename', 'PtrSync', 'PtrRename', 'PtrSync', 'PtrRename', 'PtrSync') }
        @{ Id = 'CLR'; Actions = @('RecClear') }
        @{ Id = 'CLF'; Actions = @('Fail') }
        @{ Id = 'TX'; Actions = @('StartTx("B")') }
        @{ Id = 'ING'; Actions = @('IngestRename', 'IngestSync', 'IngestRename', 'IngestSync') }
        @{ Id = 'PRP'; Actions = @('TxPrepare') }
        @{ Id = 'SW2'; Actions = @('PtrRename', 'PtrSync', 'PtrRename', 'PtrSync', 'PtrRename', 'PtrSync') }
        @{ Id = 'END'; Actions = @('TxEnd') }
        @{ Id = 'COL'; Actions = @('CollRead', 'CollSync', 'CollSync', 'CollSync', 'CollRemove', 'CollFsync', 'CollRemove', 'CollFsync') }
        @{ Id = 'PWR'; Actions = @('Crash') }
        @{ Id = 'OPN'; Actions = @('Open', 'RfRecSync') }
        @{ Id = 'RFA'; Actions = @('RfRename') }
    )
}
