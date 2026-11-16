# Copyright (c) Michael Grier.
#
# The behaviours of the specs in this directory that tools/export-trace-corpus.ps1 writes into the
# trace corpus, crates/canopy/tests/traces/, beside every timeline's replay
# (DESIGN-NOTES.md "The trace corpus").
#
#   Simulations      random behaviours of a configuration of the protocol as built, for breadth:
#                    Spec, Config, Depth (the most actions each takes), Seeds (how many
#                    behaviours, one for each seed from 1), Walks (optional, how many random
#                    walks TLC takes for a seed; 50 if not given) and Select (optional, which of
#                    them is kept: 'longest', the default, or 'distinct', the walk that takes the
#                    most kinds of action and the longest of those, for a configuration whose
#                    longest walks repeat one loop).
#                    KindState optionally names control-state fields whose transitions distinguish
#                    actions when TLC reports one enclosing action for every step.
#   Counterexamples  the error trace of a configuration whose check fails, for the defects named
#                    in DESIGN-NOTES.md: Spec, Config, Name (the defect), and Departs (optional, the
#                    step at which the code as built leaves the behaviour, as #<n> for its n-th
#                    action).
#   Derived          per spec, values a bridge needs that a configuration names only through the
#                    spec's own definitions, as name = TLA+ expression; each trace carries them
#                    evaluated in its configuration.
@{
    Simulations = @(
        @{ Spec = 'PointerPublication'; Config = 'PointerPublication'; Depth = 40; Seeds = 12 }
        @{ Spec = 'Creation'; Config = 'Creation'; Depth = 40; Seeds = 6 }
        @{ Spec = 'Creation'; Config = 'Creation-TwoCreators'; Depth = 40; Seeds = 3 }
        @{ Spec = 'Creation'; Config = 'Creation-TwoImplementations'; Depth = 40; Seeds = 3 }
        @{ Spec = 'Creation'; Config = 'Creation-DifferentInputs'; Depth = 40; Seeds = 3 }
        @{ Spec = 'Creation'; Config = 'Creation-ThreeDifferent'; Depth = 40; Seeds = 3 }
        @{ Spec = 'Creation'; Config = 'Creation-Four'; Depth = 40; Seeds = 3 }
        @{ Spec = 'Creation'; Config = 'Creation-CreatorAndOpeners'; Depth = 40; Seeds = 4 }
        @{ Spec = 'Creation'; Config = 'Creation-TwoCreatorsAndOpener'; Depth = 40; Seeds = 4 }
        @{ Spec = 'SharedPublication'; Config = 'SharedPublication'; Depth = 160; Seeds = 4; Walks = 2000; Select = 'distinct'; KindState = @('pc', 'att', 'wph', 'log', 'runPhase') }
        @{ Spec = 'SharedPublication'; Config = 'SharedPublication-RecordTxn'; Depth = 160; Seeds = 4; Walks = 2000; Select = 'distinct'; KindState = @('pc', 'att', 'wph', 'log', 'runPhase') }
        @{ Spec = 'SharedPublication'; Config = 'SharedPublication-Mixed'; Depth = 160; Seeds = 4; Walks = 2000; Select = 'distinct'; KindState = @('pc', 'att', 'wph', 'log', 'runPhase') }
        @{ Spec = 'SharedPublication'; Config = 'SharedPublication-HealthyPublication'; Depth = 160; Seeds = 4; Walks = 2000; Select = 'distinct'; KindState = @('pc', 'att', 'wph', 'log', 'runPhase') }
        @{ Spec = 'SharedPublication'; Config = 'SharedPublication-HealthyTransactions'; Depth = 160; Seeds = 4; Walks = 2000; Select = 'distinct'; KindState = @('pc', 'att', 'wph', 'log', 'runPhase') }
        @{ Spec = 'SharedPublication'; Config = 'SharedPublication-SharedPublication'; Depth = 160; Seeds = 4; Walks = 2000; Select = 'distinct'; KindState = @('pc', 'att', 'wph', 'log', 'runPhase') }
        @{ Spec = 'SharedPublication'; Config = 'SharedPublication-SharedTransactions'; Depth = 160; Seeds = 4; Walks = 2000; Select = 'distinct'; KindState = @('pc', 'att', 'wph', 'log', 'runPhase') }
        @{ Spec = 'Coordination'; Config = 'Coordination'; Depth = 60; Seeds = 4 }
        @{ Spec = 'Coordination'; Config = 'Coordination-ThreeProcs'; Depth = 60; Seeds = 2 }
        @{ Spec = 'Coordination'; Config = 'Coordination-Txn'; Depth = 60; Seeds = 2 }
        @{ Spec = 'Coordination'; Config = 'Coordination-Recovery'; Depth = 60; Seeds = 2 }
        @{ Spec = 'Coordination'; Config = 'Coordination-Registry'; Depth = 60; Seeds = 2 }
        @{ Spec = 'Coordination'; Config = 'Coordination-UpgradeBody'; Depth = 60; Seeds = 2 }
        @{ Spec = 'Coordination'; Config = 'Coordination-Notify'; Depth = 60; Seeds = 2 }
        @{ Spec = 'Coordination'; Config = 'Coordination-Writers'; Depth = 60; Seeds = 6; Walks = 2000; Select = 'distinct' }
        @{ Spec = 'Coordination'; Config = 'Coordination-WritersTxn'; Depth = 60; Seeds = 6; Walks = 2000; Select = 'distinct' }
        @{ Spec = 'Coordination'; Config = 'Coordination-WritersShared'; Depth = 60; Seeds = 6; Walks = 2000; Select = 'distinct' }
    )
    Derived = @{
        Creation = @{ scenario = 'Scenarios[Scenario]' }
    }
    Witnesses = @(
        @{ Spec = 'SharedPublication'; Config = 'SharedPublication-Witness-AWriteCollects'; Name = 'write-collects' }
        @{ Spec = 'SharedPublication'; Config = 'SharedPublication-Witness-ALiveRollbackCollects'; Name = 'rollback-collects' }
        @{ Spec = 'SharedPublication'; Config = 'SharedPublication-Witness-ARecoveryCollectsACommittedTransaction'; Name = 'recovery-collects-committed' }
        @{ Spec = 'SharedPublication'; Config = 'SharedPublication-Witness-ARecoveryCollectsARolledBackTransaction'; Name = 'recovery-collects-rollback' }
        @{ Spec = 'SharedPublication'; Config = 'SharedPublication-Witness-DirectoryDebtUnitRuns'; Name = 'directory-unit' }
        @{ Spec = 'SharedPublication'; Config = 'SharedPublication-Witness-DirectoryClearSurvivesOnlyLive'; Name = 'directory-clear-crash' }
        @{ Spec = 'SharedPublication'; Config = 'SharedPublication-Witness-StaleRecoveryRestarts'; Name = 'stale-recovery-restarts' }
    )
    Counterexamples = @(
        @{ Spec = 'PointerPublication'; Config = 'PointerPublication-NoCollectSync'; Name = 'DANGLING-DURABLE'; Departs = '#11' }
        @{ Spec = 'PointerPublication'; Config = 'PointerPublication-Control-Unguarded'; Name = 'STRANDED-VOTE'; Departs = '#10' }
    )
}
