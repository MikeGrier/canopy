\* Copyright (c) Michael Grier.
------------------------- MODULE MemFsScenarios --------------------------
(***************************************************************************)
(* Deterministic reachability scenarios for MemFsDurability. Each one is  *)
(* also a Rust bridge scenario; a witness proves that the reference model   *)
(* reaches the bug shape the bridge must reproduce against MemFs.          *)
(***************************************************************************)
EXTENDS MemFsDurability

CONSTANT Scenario
ASSUME Scenario \in {"double-name", "replace-file", "carried-move", "unnamed-child",
                     "remove-tree-unsynced", "remove-tree-synced", "remove-tree-recreate",
                     "remove-empty-recreate", "moved-descendant-sync",
                     "moved-unnamed-child", "file-over-unnamed-dir",
                     "rename-over-stale-mark", "rename-keeps-old-dst"}

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

DurableTreeSteps ==
    CASE steps = 0 -> CreateDir(1)
      [] steps = 1 -> SyncDir(1)
      [] steps = 2 -> CreateDir(6)
      [] steps = 3 -> CreateDir(11)
      [] steps = 4 -> CreateFile(12, "A")
      [] steps = 5 -> SyncFile(12)
      [] steps = 6 -> SyncDir(11)
      [] steps = 7 -> SyncDir(6)
      [] steps = 8 -> SyncDir(1)
      [] steps = 9 -> RemoveTree(6)
      [] OTHER -> UNCHANGED vars

RemoveTreeUnsyncedNext ==
    IF steps < 10 THEN DurableTreeSteps
    ELSE CASE steps = 10 -> Crash
           [] OTHER -> UNCHANGED vars

RemoveTreeSyncedNext ==
    IF steps < 10 THEN DurableTreeSteps
    ELSE CASE steps = 10 -> SyncDir(1)
           [] steps = 11 -> Crash
           [] OTHER -> UNCHANGED vars

MovedTreeSteps ==
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
      [] OTHER -> UNCHANGED vars

RemoveTreeRecreateNext ==
    IF steps < 10 THEN MovedTreeSteps
    ELSE CASE steps = 10 -> RemoveTree(6)
           [] steps = 11 -> CreateDir(6)
           [] steps = 12 -> SyncDir(1)
           [] steps = 13 -> Crash
           [] OTHER -> UNCHANGED vars

MovedDescendantSyncNext ==
    IF steps < 10 THEN MovedTreeSteps
    ELSE CASE steps = 10 -> SyncDir(11)
           [] steps = 11 -> SyncDir(1)
           [] steps = 12 -> Crash
           [] OTHER -> UNCHANGED vars

MovedUnnamedChildNext ==
    CASE steps = 0 -> CreateDir(1)
      [] steps = 1 -> SyncDir(1)
      [] steps = 2 -> CreateDir(8)
      [] steps = 3 -> SyncDir(8)
      [] steps = 4 -> SyncDir(1)
      [] steps = 5 -> CreateDir(9)
      [] steps = 6 -> CreateFile(10, "A")
      [] steps = 7 -> SyncFile(10)
      [] steps = 8 -> SyncDir(9)
      [] steps = 9 -> MoveFromTo(8, 6)
      [] steps = 10 -> SyncDir(1)
      [] steps = 11 -> Crash
      [] OTHER -> UNCHANGED vars

FileOverUnnamedDirNext ==
    CASE steps = 0 -> CreateDir(1)
      [] steps = 1 -> SyncDir(1)
      [] steps = 2 -> CreateDir(6)
      [] steps = 3 -> SyncDir(6)
      [] steps = 4 -> RemoveEmptyDir(6)
      [] steps = 5 -> CreateFile(6, "A")
      [] steps = 6 -> SyncFile(6)
      [] steps = 7 -> SyncDir(1)
      [] steps = 8 -> Crash
      [] OTHER -> UNCHANGED vars

RenameOverStaleMarkNext ==
    IF steps < 9 THEN MovedTreeSteps
    ELSE CASE steps = 9 -> CreateDir(6)
           [] steps = 10 -> CreateDir(11)
           [] steps = 11 -> SyncDir(11)
           [] steps = 12 -> RemoveEmptyDir(11)
           [] steps = 13 -> MoveFromTo(8, 6)
           [] steps = 14 -> SyncDir(1)
           [] steps = 15 -> Crash
           [] OTHER -> UNCHANGED vars

RenameKeepsOldDstNext ==
    CASE steps = 0 -> CreateDir(1)
      [] steps = 1 -> SyncDir(1)
      [] steps = 2 -> CreateDir(6)
      [] steps = 3 -> CreateDir(11)
      [] steps = 4 -> SyncDir(6)
      [] steps = 5 -> SyncDir(1)
      [] steps = 6 -> RemoveEmptyDir(11)
      [] steps = 7 -> CreateDir(8)
      [] steps = 8 -> SyncDir(1)
      [] steps = 9 -> CreateDir(9)
      [] steps = 10 -> CreateFile(10, "A")
      [] steps = 11 -> SyncFile(10)
      [] steps = 12 -> SyncDir(9)
      [] steps = 13 -> MoveFromTo(8, 6)
      [] steps = 14 -> Crash
      [] OTHER -> UNCHANGED vars

RemoveEmptyRecreateNext ==
    IF steps < 10 THEN MovedTreeSteps
    ELSE CASE steps = 10 -> RemoveFile(12)
           [] steps = 11 -> RemoveEmptyDir(11)
           [] steps = 12 -> RemoveEmptyDir(6)
           [] steps = 13 -> CreateDir(6)
           [] steps = 14 -> SyncDir(1)
           [] steps = 15 -> Crash
           [] OTHER -> UNCHANGED vars

ScenarioNext ==
    CASE Scenario = "double-name" -> DoubleNameNext
      [] Scenario = "replace-file" -> ReplaceFileNext
      [] Scenario = "carried-move" -> CarriedMoveNext
      [] Scenario = "unnamed-child" -> UnnamedChildNext
      [] Scenario = "remove-tree-unsynced" -> RemoveTreeUnsyncedNext
      [] Scenario = "remove-tree-synced" -> RemoveTreeSyncedNext
      [] Scenario = "remove-tree-recreate" -> RemoveTreeRecreateNext
      [] Scenario = "remove-empty-recreate" -> RemoveEmptyRecreateNext
      [] Scenario = "moved-descendant-sync" -> MovedDescendantSyncNext
      [] Scenario = "moved-unnamed-child" -> MovedUnnamedChildNext
      [] Scenario = "file-over-unnamed-dir" -> FileOverUnnamedDirNext
      [] Scenario = "rename-over-stale-mark" -> RenameOverStaleMarkNext
      [] Scenario = "rename-keeps-old-dst" -> RenameKeepsOldDstNext
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
ScenarioWitness_UnsyncedRemovalUndone == ~(
    /\ Scenario = "remove-tree-unsynced"
    /\ steps = 11
    /\ live[6] = Directory
    /\ live[11] = Directory
    /\ live[12].kind = "file"
    /\ liveData[live[12].inode] = "A"
)

\* Audience: internal.
ScenarioWitness_SyncedRemovalSurvives == ~(
    /\ Scenario = "remove-tree-synced"
    /\ steps = 12
    /\ live[1] = Directory
    /\ live[6] = Absent
    /\ live[11] = Absent
    /\ live[12] = Absent
)

\* Audience: internal.
ScenarioWitness_RecreatedAfterRemovalEmpty == ~(
    /\ Scenario = "remove-tree-recreate"
    /\ steps = 14
    /\ live[6] = Directory
    /\ live[11] = Absent
    /\ live[12] = Absent
    /\ live[8] = Absent
)

\* Audience: internal.
ScenarioWitness_RecreatedAfterEmptyRemovalEmpty == ~(
    /\ Scenario = "remove-empty-recreate"
    /\ steps = 16
    /\ live[6] = Directory
    /\ live[11] = Absent
    /\ live[12] = Absent
    /\ live[8] = Absent
)

\* Audience: internal.
ScenarioWitness_MovedDescendantSyncKeepsContents == ~(
    /\ Scenario = "moved-descendant-sync"
    /\ steps = 13
    /\ live[6] = Directory
    /\ live[11] = Directory
    /\ live[12].kind = "file"
    /\ liveData[live[12].inode] = "A"
    /\ live[8] = Absent
)

\* Audience: internal.
ScenarioWitness_MovedUnnamedChildLost == ~(
    /\ Scenario = "moved-unnamed-child"
    /\ steps = 12
    /\ live[6] = Directory
    /\ live[11] = Absent
    /\ live[12] = Absent
    /\ live[8] = Absent
)

\* Audience: internal.
ScenarioWitness_FileOverUnnamedDirSurvives == ~(
    /\ Scenario = "file-over-unnamed-dir"
    /\ steps = 9
    /\ live[6].kind = "file"
    /\ liveData[live[6].inode] = "A"
)

\* Audience: internal.
ScenarioWitness_RenameOverStaleMarkKeepsSubtree == ~(
    /\ Scenario = "rename-over-stale-mark"
    /\ steps = 16
    /\ live[6] = Directory
    /\ live[11] = Directory
    /\ live[12].kind = "file"
    /\ liveData[live[12].inode] = "A"
    /\ live[8] = Absent
)

\* Audience: internal.
ScenarioWitness_RenameKeepsOldDstChild == ~(
    /\ Scenario = "rename-keeps-old-dst"
    /\ steps = 15
    /\ live[6] = Directory
    /\ live[11] = Directory
    /\ live[12] = Absent
    /\ live[8] = Directory
    /\ live[9] = Absent
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
    /\ (Scenario = "remove-tree-unsynced" /\ steps = 11) =>
        /\ live[6] = Directory
        /\ live[11] = Directory
        /\ live[12].kind = "file"
        /\ liveData[live[12].inode] = "A"
    /\ (Scenario = "remove-tree-synced" /\ steps = 12) =>
        /\ live[1] = Directory
        /\ live[6] = Absent
        /\ live[11] = Absent
        /\ live[12] = Absent
    /\ (Scenario = "remove-tree-recreate" /\ steps = 14) =>
        /\ live[6] = Directory
        /\ live[11] = Absent
        /\ live[12] = Absent
        /\ live[8] = Absent
    /\ (Scenario = "remove-empty-recreate" /\ steps = 16) =>
        /\ live[6] = Directory
        /\ live[11] = Absent
        /\ live[12] = Absent
        /\ live[8] = Absent
    /\ (Scenario = "moved-descendant-sync" /\ steps = 13) =>
        /\ live[6] = Directory
        /\ live[11] = Directory
        /\ live[12].kind = "file"
        /\ liveData[live[12].inode] = "A"
        /\ live[8] = Absent
    /\ (Scenario = "moved-unnamed-child" /\ steps = 12) =>
        /\ live[6] = Directory
        /\ live[11] = Absent
        /\ live[12] = Absent
        /\ live[8] = Absent
    /\ (Scenario = "file-over-unnamed-dir" /\ steps = 9) =>
        /\ live[6].kind = "file"
        /\ liveData[live[6].inode] = "A"
    /\ (Scenario = "rename-over-stale-mark" /\ steps = 16) =>
        /\ live[6] = Directory
        /\ live[11] = Directory
        /\ live[12].kind = "file"
        /\ liveData[live[12].inode] = "A"
        /\ live[8] = Absent
    /\ (Scenario = "rename-keeps-old-dst" /\ steps = 15) =>
        /\ live[6] = Directory
        /\ live[11] = Directory
        /\ live[12] = Absent
        /\ live[8] = Directory
        /\ live[9] = Absent

=============================================================================
