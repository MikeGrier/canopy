\* Copyright (c) Michael Grier.
---------------------- MODULE CoordinationAcquisition -----------------------
(***************************************************************************)
(* The coordination model as a refinement of the acquisition model         *)
(* (Acquisition.tla; DESIGN-NOTES "The acquisition layer is a model of its *)
(* own"). A mapping of att, pc, lockHolder and upgrades names the          *)
(* acquisition state each coordination state is, and AcquisitionRefined    *)
(* states that every behaviour of this model is one of that model's under  *)
(* it: each step takes or releases writer exclusion, the gate or a tier as *)
(* one of the acquisition model's steps, or changes none of them. The      *)
(* coordination model keeps its own steps, so the controls that change the *)
(* order of takes break the rules they test; the refinement is checked in  *)
(* the main configurations only.                                           *)
(*                                                                         *)
(* Where this model is finer, the mapping says which acquisition state it  *)
(* is. Registering is the check passed and the handle not yet admitted, so *)
(* it is "tiers", and admission is the registry's last step. Unregistering *)
(* still holds the tiers: a handle being dropped is still "attached", and  *)
(* letting go of them is its exit; an upgrader's is still "draining", and  *)
(* letting go of them ends its drain. A detached handle holds nothing and  *)
(* is "exited", as is a dead one. A holder of writer exclusion is "held"   *)
(* from the take of the lock to its release, the ownership record and the  *)
(* operation under it included. An upgrade body's hold of writer           *)
(* exclusion is hidden: its upgrader holds the writer tier exclusively, so *)
(* no other handle that may write is attached to wait for exclusion, and   *)
(* the hold is uncontended. Upgrade starts are counted only where this     *)
(* model counts them, with the body (UpSteps > 0).                         *)
(***************************************************************************)
EXTENDS CoordinationLiveness

\* The acquisition model's role of each process. An upgrader is unsafe for readers when
\* the floor of its upgrade is above the oldest reader schema its layer starts with.
AcqRole(p) ==
    IF RoleOf[p] # "upgrader" THEN RoleOf[p]
    ELSE IF UpF(p) <= Oldest[UpL(p)] THEN "safeUpgrader" ELSE "unsafeUpgrader"
AcqRoles == [p \in Procs |-> AcqRole(p)]

AcqAtt ==
    [p \in Procs |->
        CASE att[p] = "registering" -> "tiers"
          [] att[p] = "unregistering" ->
                IF pc[p] = "converting" THEN "draining" ELSE "attached"
          [] OTHER -> att[p]]

AcqPc ==
    [p \in Procs |->
        IF pc[p] \in {"exited", "dead", "detached"} THEN "exited"
        ELSE IF pc[p] \in {"new", "attaching", "converting", "waiting"} THEN pc[p]
        ELSE IF att[p] = "uBody" THEN "converting"
        ELSE IF pc[p] \in {"idle", "detaching"} THEN "idle"
        ELSE "held"]

AcqLockHolder ==
    IF lockHolder # None /\ att[lockHolder] = "uBody" THEN None ELSE lockHolder

AcqUpgrades == IF UpSteps > 0 THEN upgrades ELSE 0

Acq(scenario) ==
    INSTANCE Acquisition WITH
        Scenario <- scenario,
        MaxUpgrades <- MaxUpgrades,
        ExclusiveFirst <- FALSE,
        CountUpgrades <- UpSteps > 0,
        Escapes <- Escapes,
        Refusals <- TRUE,
        Deaths <- MaxDeaths > 0 \/ MaxCrashes > 0,
        att <- AcqAtt,
        pc <- AcqPc,
        lockHolder <- AcqLockHolder,
        upgrades <- AcqUpgrades

\* The acquisition model's scenario with these roles. The table does not depend on the
\* scenario an instance is given, so any of its names serves to read it.
AcqScenarios == Acq("Writers2")!Scenarios
AcqScenario == CHOOSE s \in DOMAIN AcqScenarios : AcqScenarios[s] = AcqRoles

\* Audience: internal.
\* Every behaviour of the coordination model is one of the acquisition model's under the
\* mapping, so writer exclusion, the gate and the tiers are taken and released as that
\* model takes and releases them, and its results about who is served hold here as long
\* as every hold here ends. It is Acq(AcqScenario)!Spec written out, so that TLC names
\* a violation by a line of this definition.
AcquisitionRefined ==
    /\ Acq(AcqScenario)!Init
    /\ [][Acq(AcqScenario)!Next]_<<AcqAtt, AcqPc, AcqLockHolder, AcqUpgrades>>

=============================================================================
