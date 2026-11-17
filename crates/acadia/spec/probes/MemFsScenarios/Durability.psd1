# Copyright (c) Michael Grier.
# Mutation checks use MemFsDurability's explicit fault controls as the realization edits.
@{
    Mutants = @(
        @{
            Name = 'file-fsync-does-not-persist-data'
            Base = 'MemFsScenarios-ReplaceFile'
            Kind = 'INVARIANTS'
            Property = 'ScenarioExpected'
            Overrides = @{ Control = '"file-fsync-skips-data"' }
        }
        @{
            Name = 'directory-fsync-does-not-persist-name'
            Base = 'MemFsScenarios-DoubleName'
            Kind = 'INVARIANTS'
            Property = 'ScenarioExpected'
            Overrides = @{ Control = '"dir-fsync-fails-to-name"' }
        }
        @{
            Name = 'carried-refresh-drops-root-descendants'
            Base = 'MemFsScenarios-CarriedMove'
            Kind = 'INVARIANTS'
            Property = 'ScenarioExpected'
            Overrides = @{ Control = '"rename-loses-carried"' }
        }
        @{
            Name = 'crash-keeps-pending-live-tree'
            Base = 'MemFsScenarios-UnnamedChild'
            Kind = 'INVARIANTS'
            Property = 'ScenarioExpected'
            Overrides = @{ Control = '"crash-keeps-live"' }
        }
    )
}
