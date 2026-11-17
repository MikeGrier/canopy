\* Copyright (c) Michael Grier.
------------------------- MODULE MemFsScenarios --------------------------
(***************************************************************************)
(* Deterministic reachability scenarios for MemFsDurability. Each one is  *)
(* also a Rust bridge scenario; a witness proves that the reference model   *)
(* reaches the bug shape the bridge must reproduce against MemFs.          *)
(***************************************************************************)
EXTENDS MemFsDurability

CONSTANT Scenario
ASSUME Scenario \in {"double-name", "replace-file", "carried-move", "unnamed-child"}

MoveFromTo(from, to) == Move(CHOOSE move \in AvailableMoves :
    move.from = from /\ move.to = to)

DoubleNameNext ==
    CASE steps = 0 -> CreateDir(1)
      [] steps = 1 -> SyncDir(1)
      [] steps = 2 -> CreateDir(2)
      [] steps = 3 -> CreateDir(3)
      [] steps = 4 -> SyncDir(1)
      [] steps = 5 -> CreateFile(4, "A")
      [] steps = 6 -> SyncFile(4)
      [] steps = 7 -> SyncDir(2)
      [] steps = 8 -> MoveFromTo(4, 5)
      [] steps = 9 -> SyncDir(3)
      [] steps = 10 -> Crash
      [] OTHER -> UNCHANGED vars

ReplaceFileNext ==
    CASE steps = 0 -> CreateDir(1)
      [] steps = 1 -> SyncDir(1)
      [] steps = 2 -> CreateFile(6, "A")
      [] steps = 3 -> SyncFile(6)
      [] steps = 4 -> SyncDir(1)
      [] steps = 5 -> RemoveFile(6)
      [] steps = 6 -> CreateDir(6)
      [] steps = 7 -> CreateFile(11, "B")
      [] steps = 8 -> SyncFile(11)
      [] steps = 9 -> SyncDir(6)
      [] steps = 10 -> SyncDir(1)
      [] steps = 11 -> Crash
      [] OTHER -> UNCHANGED vars

CarriedMoveNext ==
    CASE steps = 0 -> CreateDir(1)
      [] steps = 1 -> SyncDir(1)
      [] steps = 2 -> CreateDir(8)
      [] steps = 3 -> CreateDir(9)
      [] steps = 4 -> CreateFile(10, "A")
      [] steps = 5 -> SyncFile(10)
      [] steps = 6 -> SyncDir(9)
      [] steps = 7 -> SyncDir(8)
      [] steps = 8 -> SyncDir(1)
      [] steps = 9 -> MoveFromTo(8, 6)
      [] steps = 10 -> SyncDir(6)
      [] steps = 11 -> SyncDir(1)
      [] steps = 12 -> Crash
      [] OTHER -> UNCHANGED vars

UnnamedChildNext ==
    CASE steps = 0 -> CreateDir(1)
      [] steps = 1 -> SyncDir(1)
      [] steps = 2 -> CreateDir(6)
      [] steps = 3 -> CreateDir(13)
      [] steps = 4 -> CreateFile(14, "A")
      [] steps = 5 -> SyncFile(14)
      [] steps = 6 -> SyncDir(13)
      [] steps = 7 -> SyncDir(1)
      [] steps = 8 -> Crash
      [] OTHER -> UNCHANGED vars

ScenarioNext ==
    CASE Scenario = "double-name" -> DoubleNameNext
      [] Scenario = "replace-file" -> ReplaceFileNext
      [] Scenario = "carried-move" -> CarriedMoveNext
      [] Scenario = "unnamed-child" -> UnnamedChildNext
      [] OTHER -> UNCHANGED vars

ScenarioSpec == Init /\ [][ScenarioNext]_vars

\* Audience: internal.
ScenarioWitness_DoubleNameAfterCrash == ~(
    /\ Scenario = "double-name"
    /\ steps = 11
    /\ live[4] = live[5]
    /\ live[4].kind = "file"
)

\* Audience: internal.
ScenarioWitness_FileReplacedByDirectory == ~(
    /\ Scenario = "replace-file"
    /\ steps = 12
    /\ live[6] = Directory
    /\ live[11].kind = "file"
    /\ liveData[live[11].inode] = "B"
)

\* Audience: internal.
ScenarioWitness_CarriedMoveSurvives == ~(
    /\ Scenario = "carried-move"
    /\ steps = 13
    /\ live[6] = Directory
    /\ live[11] = Directory
    /\ live[12].kind = "file"
    /\ liveData[live[12].inode] = "A"
    /\ live[8] = Absent
)

\* Audience: internal.
ScenarioWitness_UnnamedChildLost == ~(
    /\ Scenario = "unnamed-child"
    /\ steps = 9
    /\ live[6] = Directory
    /\ live[13] = Absent
    /\ live[14] = Absent
)

\* Audience: internal.
ScenarioExpected ==
    /\ (Scenario = "double-name" /\ steps = 11) =>
        /\ live[4] = live[5]
        /\ live[4].kind = "file"
    /\ (Scenario = "replace-file" /\ steps = 12) =>
        /\ live[6] = Directory
        /\ live[11].kind = "file"
        /\ liveData[live[11].inode] = "B"
    /\ (Scenario = "carried-move" /\ steps = 13) =>
        /\ live[6] = Directory
        /\ live[11] = Directory
        /\ live[12].kind = "file"
        /\ liveData[live[12].inode] = "A"
        /\ live[8] = Absent
    /\ (Scenario = "unnamed-child" /\ steps = 9) =>
        /\ live[6] = Directory
        /\ live[13] = Absent
        /\ live[14] = Absent

=============================================================================
