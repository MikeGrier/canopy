# Copyright (c) Michael Grier.
# Probes of the registry of attached handles.
# tools/run-probes.ps1 describes the fields and how a mutant stays current.
@{
    Mutants = @(
        @{
            Name      = 'die-keeps-registry-locks'
            Base      = 'Coordination-Witness-ADeathBetweenTheClaimAndLivenessLocks'
            Kind      = 'INVARIANTS'
            Property  = 'RegistryLocksAreHeldOnlyByLiveProcesses'
            Def       = 'Die'
            Old = @'
    /\ cl' = [s \in Slots |-> IF cl[s] = p THEN None ELSE cl[s]]
    /\ ll' = [s \in Slots |-> IF ll[s] = p THEN None ELSE ll[s]]
'@
            New = @'
    /\ cl' = cl
    /\ ll' = ll
'@
        }

        @{
            Name      = 'exit-skips-unregister'
            Base      = 'Coordination-Witness-AHandleUnregistersAtExit'
            Kind      = 'INVARIANTS'
            Property  = 'EntriesFollowAttachment'
            Def       = 'Exit'
            Old = @'
    /\ IF pc[p] = "idle" /\ RegistryOn
'@
            New = @'
    /\ IF FALSE
'@
        }

        @{
            Name      = 'overflow-unannounced'
            Base      = 'Coordination-Witness-AHandleAttachesUnregistered'
            Kind      = 'INVARIANTS'
            Property  = 'AnUnregisteredHandleIsAnnounced'
            Def       = 'RegOverflow'
            Old = @'
    /\ ovf' = TRUE
'@
            New = @'
    /\ ovf' = ovf
'@
        }

        @{
            Name      = 'listing-ignores-unknown'
            Base      = 'Coordination-Witness-AListingMeetsAClaimedEntry'
            Kind      = 'PROPERTIES'
            Property  = 'AListingSaysWhenItMayBeIncomplete'
            Def       = 'ListDone'
            Old = @'
                   IF lunk[p] \/ (IF Control = "listing-infers-from-fullness"
'@
            New = @'
                   IF FALSE \/ (IF Control = "listing-infers-from-fullness"
'@
        }

        @{
            Name      = 'update-without-claim'
            Base      = 'Coordination-Witness-AnOwnerUpdatesItsEntry'
            Kind      = 'PROPERTIES'
            Property  = 'EntryChangesAreMadeUnderTheClaimLock'
            Def       = 'UpdTake'
            Old = @'
    /\ cl' = Set(cl, rcur[p], p)
    /\ rst' = Set(rst, p, "upd")
'@
            New = @'
    /\ cl' = cl
    /\ rst' = Set(rst, p, "upd")
'@
        }

        @{
            Name      = 'unregister-without-claim'
            Base      = 'Coordination-Witness-AHandleUnregistersAtExit'
            Kind      = 'PROPERTIES'
            Property  = 'EntryChangesAreMadeUnderTheClaimLock'
            Def       = 'UnregClaim'
            Old = @'
    /\ cl' = Set(cl, rcur[p], p)
    /\ rst' = Set(rst, p, "uc")
'@
            New = @'
    /\ cl' = cl
    /\ rst' = Set(rst, p, "uc")
'@
        }

        @{
            Name      = 'probe-ignores-liveness'
            Base      = 'Coordination-Witness-AListingSeesADeadEntry'
            Kind      = 'PROPERTIES'
            Property  = 'LiveReportIsJustified'
            Def       = 'ListProbe'
            Old = @'
                     ELSE IF ll[lcur[p]] = None THEN "dead" ELSE "live">>)
'@
            New = @'
                     ELSE "live">>)
'@
        }

        @{
            Name      = 'registrant-keeps-no-liveness'
            Base      = 'Coordination-Witness-AHandleRegisters'
            Kind      = 'INVARIANTS'
            Property  = 'ARegisteredHandleOwnsItsEntry'
            Def       = 'RegLive'
            Old = @'
          THEN /\ ll' = IF ll[rcur[p]] = None THEN Set(ll, rcur[p], p) ELSE ll
'@
            New = @'
          THEN /\ ll' = ll
'@
        }

        @{
            Name      = 'crash-keeps-entries'
            Base      = 'Coordination-RegistryCrash'
            Kind      = 'PROPERTIES'
            Property  = 'ACrashDiscardsTheRegistry'
            Def       = 'Crash'
            Old = @'
    /\ etok' = [s \in Slots |-> None]
'@
            New = @'
    /\ etok' = etok
'@
        }

        @{
            Name      = 'die-clears-entry'
            Base      = 'Coordination-Witness-ADeathWhileRegistered'
            Kind      = 'PROPERTIES'
            Property  = 'DeathLeavesTheFilesystemAsItWas'
            Def       = 'Die'
            Old = @'
UNCHANGED <<rdata>>
'@
            New = @'
etok' = [s \in Slots |-> IF etok[s] = p THEN None ELSE etok[s]]
    /\ UNCHANGED <<edr>>
'@
        }

        @{
            Name      = 'registrant-steals-claim'
            Base      = 'Coordination-Witness-AListingMeetsAClaimedEntry'
            Kind      = 'PROPERTIES'
            Property  = 'EntryInspectionsAreMadeUnderTheClaimLock'
            Def       = 'RegClaim'
            Old = @'
    /\ IF cl[s] = None \/ Control = "register-without-claim"
'@
            New = @'
    /\ IF TRUE
'@
        }

        @{
            Name      = 'iso-list-inspects-without-claim'
            Base      = 'Coordination-Control-ListWithoutClaim'
            Kind      = 'PROPERTIES'
            Property  = 'EntryInspectionsAreMadeUnderTheClaimLock'
        }

        @{
            Name      = 'iso-reuse-leaves-no-owner'
            Base      = 'Coordination-Control-ReuseWithoutLivenessProbe'
            Kind      = 'INVARIANTS'
            Property  = 'ARegisteredHandleOwnsItsEntry'
        }

        @{
            Name      = 'iso-early-release-changes-without-claim'
            Base      = 'Coordination-Control-RegisterReleasesClaimEarly'
            Kind      = 'PROPERTIES'
            Property  = 'EntryChangesAreMadeUnderTheClaimLock'
        }

        @{
            Name      = 'equiv-unregister-keeps-token'
            Base      = @('Coordination-Registry', 'Coordination-RegistryUpgradeFull', 'Coordination-RegistryCrash', 'Coordination-RegistryRoomy')
            Expect    = 'passes'
            Def       = 'UnregClear'
            Old = @'
    /\ etok' = Set(etok, rcur[p], None)
'@
            New = @'
    /\ etok' = etok
'@
        }
    )
}
