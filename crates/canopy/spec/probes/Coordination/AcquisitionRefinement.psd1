# Copyright (c) Michael Grier.
# Probes of the refinement of the acquisition model: a step that takes or releases the gate or a
# tier other than as Acquisition.tla does is caught by AcquisitionRefined alone. Where a process
# may die, a step that lets go of everything a process holds is a death, so a mutant of that
# shape runs where none dies.
# tools/run-probes.ps1 describes the fields and how a mutant stays current.
@{
    Mutants = @(
        @{
            Name      = 'writer-skips-the-writer-tier'
            Base      = 'Coordination-Attachment'
            Kind      = 'PROPERTIES'
            Property  = 'AcquisitionRefined'
            Def       = 'DropGate'
            Old = @'
    /\ att' = Set(att, p, IF CanWrite(p) THEN "rd" ELSE "tiers")
'@
            New = @'
    /\ att' = Set(att, p, "tiers")
'@
        }

        @{
            Name      = 'give-up-while-holding-the-reader-tier'
            Base      = 'Coordination-RegistryThreeProcs'
            Kind      = 'PROPERTIES'
            Property  = 'AcquisitionRefined'
            Def       = 'GiveUp'
            Old = @'
    /\ \/ att[p] \in {"wGate", "hasGate", "gateWr", "rd", "rdChecked"}
'@
            New = @'
    /\ \/ att[p] \in {"wGate", "hasGate", "gateWr", "gateRd", "rd", "rdChecked"}
'@
        }
    )
}
