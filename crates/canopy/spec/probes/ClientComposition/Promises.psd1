# Copyright (c) Michael Grier.
# Mutation checks for the projection promises composed by ClientComposition.
@{
    Mutants = @(
        @{
            Name = 'definite-no-effect-changes-contents'
            Base = 'ClientComposition'
            Kind = 'PROPERTIES'
            Property = 'ClientRefined'
            Def = 'FinishStep'
            Old = @'
                /\ clientContents' = IF Control = "outcome"
                                     THEN value
                                     ELSE IF effect = DefinitelyNoEffect
                                     THEN clientContents
                                     ELSE value
'@
            New = @'
                /\ clientContents' = IF Control = "outcome"
                                     THEN value
                                     ELSE IF effect = DefinitelyNoEffect
                                     THEN value
                                     ELSE value
'@
        }
        @{
            Name = 'pair-loss-reveals-definite-no-effect'
            Base = 'ClientComposition'
            Kind = 'PROPERTIES'
            Property = 'ClientRefined'
            Def = 'ClientProjectedEntry'
            Old = @'
        effect |-> IF state = "unavailable"
                   THEN IF Control = "ambiguity" THEN DefinitelyNoEffect ELSE EffectUnknown
                   ELSE outcome.effect,
'@
            New = @'
        effect |-> IF state = "unavailable"
                   THEN DefinitelyNoEffect
                   ELSE outcome.effect,
'@
        }
        @{
            Name = 'later-request-starts-before-earlier-request'
            Base = 'ClientComposition'
            Kind = 'PROPERTIES'
            Property = 'ClientRefined'
            Overrides = @{ CompletionCapacity = '2' }
            Edits = @(
                @{
                    Def = 'CompositionSpec'
                    Old = @'
CompositionSpec == CompositionInit /\ [][CompositionNext]_compositionVars
'@
                    New = @'
CompositionSpec == CompositionInit /\ [][CompositionCheckNext]_compositionVars
'@
                }
                @{
                    Def = 'FaultStartOutOfOrder'
                    Old = @'
    /\ Control = "order"
'@
                    New = @'
    /\ TRUE
'@
                }
            )
        }
        @{
            Name = 'ring-skips-the-next-ready-pair'
            Base = 'ClientComposition'
            Kind = 'INVARIANTS'
            Property = 'ReadyPairService'
            Edits = @(
                @{
                    Def = 'CompositionSpec'
                    Old = @'
CompositionSpec == CompositionInit /\ [][CompositionNext]_compositionVars
'@
                    New = @'
CompositionSpec == CompositionInit /\ [][CompositionCheckNext]_compositionVars
'@
                }
                @{
                    Def = 'FaultSkipReadyPair'
                    Old = @'
    /\ Control = "ready-pair"
'@
                    New = @'
    /\ TRUE
'@
                }
            )
        }
    )
}
