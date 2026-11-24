@{
    Timeline = 'T32'
    Base = 'PointerPublication'
    Note = 'A repair renames a missing replica, fails before syncing it, and a later collection syncs the live entry before deleting anything.'
    Steps = @(
        @{ Id = 'DMG'; Actions = @('Damage(2)') }
        @{ Id = 'REP'; Actions = @('StartOp("repair")', 'RfRecSync', 'RepRead', 'RepRename', 'RepRename', 'Fail') }
        @{ Id = 'COL'; Actions = @('StartOp("collect")', 'RfRecSync', 'CollRead', 'CollSync', 'CollSync', 'CollSync') }
    )
}
