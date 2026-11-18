# Copyright (c) Michael Grier.
# Probes of the creation reservation.
# tools/run-probes.ps1 describes the fields and how a mutant stays current.
@{
    Mutants = @(
        @{
            Name      = 'ResumerSkipsReservationSync'
            Base      = 'Creation'
            Overrides = @{ Procs = '{1, 2}'; NArt = '2'; MaxTerminations = '1'; MaxCrashes = '1'; Intrusion = 'TRUE'; Scenario = '"TwoCreators"' }
            Def       = 'SyncRes'
            Old = @'
rdur' = IF Control = "reservation-not-synced" THEN rdur ELSE TRUE
'@
            New = @'
rdur' = IF Control = "reservation-not-synced" \/ path[p] = "resume" THEN rdur ELSE TRUE
'@
        }

        @{
            Name      = 'ResumerSkipsArtifactSync'
            Base      = 'Creation'
            Overrides = @{ Procs = '{1, 2}'; NArt = '2'; MaxTerminations = '1'; MaxCrashes = '1'; Intrusion = 'TRUE'; Scenario = '"TwoCreators"' }
            Def       = 'SyncArt'
            Old = @'
adur' = IF Control = "body-not-synced-before-marker" /\ ak[p] < Marker
'@
            New = @'
adur' = IF path[p] = "resume" \/ (Control = "body-not-synced-before-marker" /\ ak[p] < Marker)
'@
        }

        @{
            Name      = 'OpenerSkipsExclusion'
            Base      = 'Creation'
            Expect    = 'passes'
            Overrides = @{ Procs = '{1, 2, 3}'; NArt = '2'; MaxTerminations = '1'; MaxCrashes = '1'; Intrusion = 'TRUE'; Scenario = '"CreatorAndOpeners"' }
            Def       = 'OpenLook'
            Old = @'
/\ pc[p] = "new" /\ RoleOf[p] = "opener" /\ cx[ImplOf[p]] = None
'@
            New = @'
/\ pc[p] = "new" /\ RoleOf[p] = "opener"
'@
        }

        @{
            Name      = 'CrashKeepsEveryone'
            Base      = 'Creation'
            Overrides = @{ Procs = '{1, 2}'; NArt = '2'; MaxTerminations = '0'; MaxCrashes = '1'; Intrusion = 'TRUE'; Scenario = '"TwoCreators"' }
            Def       = 'Crash'
            Old = @'
/\ res' \in IF rdur \/ res.st = "none" THEN {res} ELSE {res, NoRes}
'@
            New = @'
/\ res' \in {res, NoRes}
'@
        }

        @{
            Name      = 'ResumerSkipsTheOccupiedRecheck'
            Base      = 'Creation'
            Overrides = @{ Procs = '{1, 2}'; NArt = '2'; MaxTerminations = '1'; MaxCrashes = '0'; Intrusion = 'TRUE'; Scenario = '"TwoCreators"' }
            Def       = 'Reconfirm'
            Old = @'
(~MarkerLive /\ res = seen[p] /\ Unoccupied))
'@
            New = @'
(~MarkerLive /\ res = seen[p]))
'@
        }

        @{
            Name      = 'CreatorSkipsTheOccupiedRecheck'
            Base      = 'Creation'
            Overrides = @{ Procs = '{1}'; NArt = '2'; MaxTerminations = '0'; MaxCrashes = '0'; Intrusion = 'TRUE'; Scenario = '"OneCreator"' }
            Def       = 'Publish'
            Old = @'
/\ pc[p] = "holdsX" /\ path[p] = "fresh" /\ CanPublish /\ Unoccupied
'@
            New = @'
/\ pc[p] = "holdsX" /\ path[p] = "fresh" /\ CanPublish
'@
        }

        @{
            Name      = 'FinderIgnoresTheInputs'
            Base      = 'Creation'
            Kind      = 'PROPERTIES'
            Property  = 'AStoreIsFoundOnlyWhereItIs'
            Overrides = @{ Procs = '{1, 2}'; NArt = '2'; MaxTerminations = '0'; MaxCrashes = '0'; Intrusion = 'FALSE'; Scenario = '"DifferentInputs"' }
            Def       = 'FindsItsStore'
            Old = @'
Identical(p) /\ Control # "stop-at-the-store"
'@
            New = @'
res.st = "whole" /\ res.impl = ImplOf[p] /\ Control # "stop-at-the-store"
'@
        }

        @{
            Name      = 'FinderIgnoresTheImplementation'
            Base      = 'Creation'
            Kind      = 'PROPERTIES'
            Property  = 'AStoreIsFoundOnlyWhereItIs'
            Overrides = @{ Procs = '{1, 2}'; NArt = '2'; MaxTerminations = '0'; MaxCrashes = '0'; Intrusion = 'FALSE'; Scenario = '"TwoImplementations"' }
            Def       = 'FindsItsStore'
            Old = @'
Identical(p) /\ Control # "stop-at-the-store"
'@
            New = @'
res.st = "whole" /\ res.inp = InOf[p] /\ Control # "stop-at-the-store"
'@
        }

        @{
            Name      = 'FinderSkipsTheMarkerSync'
            Base      = 'Creation'
            Kind      = 'INVARIANTS'
            Property  = 'AnOpenedStoreKeepsItsMarker'
            Overrides = @{ Procs = '{1, 2}'; NArt = '2'; MaxTerminations = '1'; MaxCrashes = '0'; Intrusion = 'FALSE'; Scenario = '"TwoCreators"' }
            Def       = 'FindStore'
            Old = @'
/\ adur' = Set(adur, Marker, TRUE)
'@
            New = @'
/\ adur' = adur
'@
        }
    )
}
