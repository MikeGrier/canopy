@{
    Timeline = 'T26'
    Base = 'PointerPublication-MaintenanceRecordLostReplica'
    Note = 'Ingesting a blob renames and syncs it, then syncs its directory.'
    Steps = @(
        @{ Id = 'LOS'; Actions = @('Damage(2)') }
        @{ Id = 'WRT'; Actions = @('StartWrite("B")', 'RfRecSync') }
        @{ Id = 'ING'; Actions = @('IngestRename', 'IngestSync', 'IngestRename', 'IngestSync') }
        @{ Id = 'REC'; Actions = @('RecWrite', 'RecSync') }
        @{ Id = 'PRN'; Actions = @('PtrRename') }
        @{ Id = 'FAI'; Actions = @('Fail') }
    )
}
