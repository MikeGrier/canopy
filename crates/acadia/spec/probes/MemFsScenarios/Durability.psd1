# Copyright (c) Michael Grier.
# Mutation checks use MemFsDurability's explicit fault controls as the realization edits.
@{
    Mutants = @(
        @{
            Name = 'rename-ignores-source-kind'
            Base = 'MemFsScenarios-RenameOverFile'
            Kind = 'INVARIANTS'
            Property = 'ScenarioExpected'
            Overrides = @{ Control = '"rename-ignores-kind"' }
        }
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
        @{
            Name = 'recursive-removal-keeps-carried-snapshot'
            Base = 'MemFsScenarios-RemoveTreeRecreate'
            Kind = 'INVARIANTS'
            Property = 'ScenarioExpected'
            Overrides = @{ Control = '"removal-keeps-carried"' }
        }
        @{
            Name = 'empty-removal-keeps-carried-snapshot'
            Base = 'MemFsScenarios-RemoveEmptyRecreate'
            Kind = 'INVARIANTS'
            Property = 'ScenarioExpected'
            Overrides = @{ Control = '"removal-keeps-carried"' }
        }
        @{
            Name = 'recursive-removal-leaves-descendants'
            Base = 'MemFsScenarios-RemoveTreeSynced'
            Kind = 'INVARIANTS'
            Property = 'TreeShape'
            Overrides = @{ Control = '"remove-tree-leaves-descendants"' }
        }
        @{
            Name = 'removal-leaves-its-path-unreplaced'
            Base = 'MemFsScenarios-RemoveTreeRecreateInPlace'
            Kind = 'INVARIANTS'
            Property = 'ScenarioExpected'
            Overrides = @{ Control = '"removal-keeps-the-path"' }
        }
        @{
            Name = 'recursive-removal-is-durable-without-sync'
            Base = 'MemFsScenarios-RemoveTreeUnsynced'
            Kind = 'INVARIANTS'
            Property = 'ScenarioExpected'
            Overrides = @{ Control = '"remove-tree-is-durable"' }
        }
    )
}
