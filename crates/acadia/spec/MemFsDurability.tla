\* Copyright (c) Michael Grier.
----------------------- MODULE MemFsDurability ---------------------------
(***************************************************************************)
(* A finite durability model of MemFs. Paths are abstract identities; Parent *)
(* names their tree relation and Moves names the subtree rebasings a rename  *)
(* may perform. Files carry inode identity, while directory fsync reconciles *)
(* names without syncing child data. Carried snapshots preserve durable      *)
(* directory contents across a rename or while an ancestor still names a    *)
(* file. Removing a directory, empty or not, drops the carried snapshots at  *)
(* or beneath it and leaves the durable tree to its parent's next sync. A    *)
(* crash replaces live names and bytes with their durable state.             *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, Sequences

CONSTANTS
    Paths,
    Values,
    MaxSteps,
    Control

ASSUME Paths \subseteq 1..14
ASSUME Values # {}
ASSUME MaxSteps \in Nat
ASSUME Control \in {"none", "file-fsync-skips-data", "dir-fsync-fails-to-name",
                    "rename-loses-carried", "crash-keeps-live",
                    "removal-keeps-carried", "remove-tree-leaves-descendants",
                    "remove-tree-is-durable"}

Absent == [kind |-> "absent", inode |-> 0]
Directory == [kind |-> "dir", inode |-> 0]
File(inode) == [kind |-> "file", inode |-> inode]
Node == {Absent, Directory} \cup
       {[kind |-> "file", inode |-> inode] : inode \in 1..MaxSteps}
None == "none"
NoSnapshot == [p \in Paths |-> Absent]

VARIABLES
    live,
    durable,
    liveData,
    durableData,
    carried,
    replaced,
    nextInode,
    steps

vars == <<live, durable, liveData, durableData, carried, replaced, nextInode, steps>>

Init ==
    /\ live = [p \in Paths |-> Absent]
    /\ durable = [p \in Paths |-> Absent]
    /\ liveData = [i \in 1..MaxSteps |-> None]
    /\ durableData = [i \in 1..MaxSteps |-> None]
    /\ carried = [p \in Paths |-> NoSnapshot]
    /\ replaced = {}
    /\ nextInode = 1
    /\ steps = 0

Parent(p) ==
    CASE p = 1 -> 0
      [] p \in {2, 3, 6, 7, 8} -> 1
      [] p = 4 -> 2
      [] p = 5 -> 3
      [] p = 9 -> 8
      [] p = 10 -> 9
      [] p \in {11, 13} -> 6
      [] p = 12 -> 11
      [] p = 14 -> 13
      [] OTHER -> 0

DirectChildren(p) == {q \in Paths : Parent(q) = p}

Below(root) == {q \in Paths :
    \/ q = root
    \/ Parent(q) = root
    \/ Parent(q) \in DirectChildren(root)
    \/ \E middle \in Paths : Parent(q) = middle /\ Parent(middle) \in DirectChildren(root)}

Moves == {
    [from |-> 8, to |-> 6,
     pairs |-> {[old |-> 8, new |-> 6], [old |-> 9, new |-> 11],
                [old |-> 10, new |-> 12]}],
    [from |-> 4, to |-> 5, pairs |-> {[old |-> 4, new |-> 5]}],
    [from |-> 9, to |-> 13,
     pairs |-> {[old |-> 9, new |-> 13], [old |-> 10, new |-> 14]}]
}

PresentSubtree(root, state) == [p \in Paths |->
    IF p \in Below(root) THEN state[p] ELSE Absent]

Rebase(snapshot, move) == [p \in Paths |->
    IF \E pair \in move.pairs : pair.old = p
    THEN LET pair == CHOOSE pair \in move.pairs : pair.old = p
         IN snapshot[pair.new]
    ELSE Absent]

MoveSnapshot(from, move) ==
    LET old == IF \E p \in Paths : carried[from][p] # Absent
               THEN carried[from]
               ELSE PresentSubtree(from, durable)
    IN [p \in Paths |->
        IF \E pair \in move.pairs : pair.new = p
        THEN LET pair == CHOOSE pair \in move.pairs : pair.new = p
             IN old[pair.old]
        ELSE Absent]

SnapshotEmpty(snapshot) == \A p \in Paths : snapshot[p] = Absent

Materialize(root, snapshot, state) == [p \in Paths |->
    IF p \in Below(root) THEN snapshot[p]
    ELSE state[p]]

DropCarriedUnder(p) ==
    IF Control = "removal-keeps-carried" THEN carried
    ELSE [q \in Paths |-> IF q \in Below(p) THEN NoSnapshot ELSE carried[q]]

ParentLive(parent) == IF parent = 0 THEN TRUE ELSE live[parent] = Directory

CreateDir(p) ==
    /\ steps < MaxSteps
    /\ live[p] = Absent
    /\ ParentLive(Parent(p))
    /\ live' = [live EXCEPT ![p] = Directory]
    /\ steps' = steps + 1
    /\ UNCHANGED <<durable, liveData, durableData, carried, replaced, nextInode>>

CreateFile(p, value) ==
    /\ steps < MaxSteps
    /\ value \in Values
    /\ live[p] = Absent
    /\ ParentLive(Parent(p))
    /\ nextInode <= MaxSteps
    /\ live' = [live EXCEPT ![p] = File(nextInode)]
    /\ liveData' = [liveData EXCEPT ![nextInode] = value]
    /\ nextInode' = nextInode + 1
    /\ steps' = steps + 1
    /\ UNCHANGED <<durable, durableData, carried, replaced>>

WriteFile(p, value) ==
    /\ steps < MaxSteps
    /\ value \in Values
    /\ live[p].kind = "file"
    /\ liveData' = [liveData EXCEPT ![live[p].inode] = value]
    /\ steps' = steps + 1
    /\ UNCHANGED <<live, durable, durableData, carried, replaced, nextInode>>

RemoveFile(p) ==
    /\ steps < MaxSteps
    /\ live[p].kind = "file"
    /\ live' = [live EXCEPT ![p] = Absent]
    /\ steps' = steps + 1
    /\ UNCHANGED <<durable, liveData, durableData, carried, replaced, nextInode>>

RemoveEmptyDir(p) ==
    /\ steps < MaxSteps
    /\ live[p] = Directory
    /\ \A q \in DirectChildren(p) : live[q] = Absent
    /\ live' = [live EXCEPT ![p] = Absent]
    /\ carried' = DropCarriedUnder(p)
    /\ steps' = steps + 1
    /\ UNCHANGED <<durable, liveData, durableData, replaced, nextInode>>

RemoveTree(p) ==
    /\ steps < MaxSteps
    /\ live[p] = Directory
    /\ live' = IF Control = "remove-tree-leaves-descendants"
               THEN [live EXCEPT ![p] = Absent]
               ELSE [q \in Paths |-> IF q \in Below(p) THEN Absent ELSE live[q]]
    /\ durable' = IF Control = "remove-tree-is-durable"
                  THEN [q \in Paths |-> IF q \in Below(p) THEN Absent ELSE durable[q]]
                  ELSE durable
    /\ carried' = DropCarriedUnder(p)
    /\ steps' = steps + 1
    /\ UNCHANGED <<liveData, durableData, replaced, nextInode>>

SyncFile(p) ==
    /\ steps < MaxSteps
    /\ live[p].kind = "file"
    /\ durableData' = IF Control = "file-fsync-skips-data"
                      THEN durableData
                      ELSE [durableData EXCEPT ![live[p].inode] = liveData[live[p].inode]]
    /\ steps' = steps + 1
    /\ UNCHANGED <<live, durable, liveData, carried, replaced, nextInode>>

RemoveSubtree(state, root) == [p \in Paths |->
    IF p \in Below(root) THEN Absent ELSE state[p]]

PlaceSnapshot(root, snapshot, state) == [p \in Paths |->
    IF p \in Below(root) /\ snapshot[p] # Absent THEN snapshot[p] ELSE
    IF p \in Below(root) THEN Absent ELSE state[p]]

BaseForChild(p) ==
    IF Control = "rename-loses-carried"
    THEN NoSnapshot
    ELSE IF ~SnapshotEmpty(carried[p])
    THEN carried[p]
    ELSE IF durable[p] = Directory
    THEN PresentSubtree(p, durable)
    ELSE NoSnapshot

HasSnapshotBelow(p, q) ==
    \E child \in DirectChildren(p) :
        /\ live[child] = Directory
        /\ q \in Below(child)
        /\ ~SnapshotEmpty(BaseForChild(child))

SnapshotBelow(p, q) ==
    LET child == CHOOSE child \in DirectChildren(p) :
        /\ live[child] = Directory
        /\ q \in Below(child)
        /\ ~SnapshotEmpty(BaseForChild(child))
    IN BaseForChild(child)[q]

ClearedBelow(p, q) ==
    \E child \in DirectChildren(p) :
        /\ live[child] # Directory
        /\ q # child
        /\ q \in Below(child)

SyncedEntry(p, q) ==
    CASE q = p -> Directory
      [] Parent(q) = p ->
          IF Control = "dir-fsync-fails-to-name" THEN durable[q]
          ELSE IF live[q] = Absent THEN Absent
          ELSE IF live[q] = Directory THEN
              IF ~SnapshotEmpty(BaseForChild(q)) THEN BaseForChild(q)[q] ELSE Directory
          ELSE live[q]
      [] ClearedBelow(p, q) -> Absent
      [] HasSnapshotBelow(p, q) -> SnapshotBelow(p, q)
      [] OTHER -> durable[q]

OwnNeedsCarried(p) ==
    Parent(p) # 0 /\ durable[p] # Directory

RefreshedHasBelow(p, q) ==
    \E child \in DirectChildren(p) :
        /\ live[child] = Directory
        /\ q \in Below(child)
        /\ ~SnapshotEmpty(BaseForChild(child))

RefreshedBelow(p, q) ==
    LET child == CHOOSE child \in DirectChildren(p) :
        /\ live[child] = Directory
        /\ q \in Below(child)
        /\ ~SnapshotEmpty(BaseForChild(child))
    IN BaseForChild(child)[q]

RefreshedSnapshot(p) == [q \in Paths |->
    IF q = p THEN Directory
    ELSE IF Parent(q) = p THEN live[q]
    ELSE IF RefreshedHasBelow(p, q) THEN RefreshedBelow(p, q)
    ELSE IF carried[p][q] # Absent THEN carried[p][q]
    ELSE Absent]

SyncIntoCarried(p) ==
    /\ OwnNeedsCarried(p)
    /\ carried' = [carried EXCEPT ![p] = RefreshedSnapshot(p)]
    /\ UNCHANGED <<live, durable, liveData, durableData, replaced, nextInode>>

SyncIntoDurable(p) ==
    /\ ~OwnNeedsCarried(p)
    /\ durable' = [q \in Paths |-> SyncedEntry(p, q)]
    /\ carried' = [q \in Paths |->
        IF q \in DirectChildren(p) THEN NoSnapshot ELSE carried[q]]
    /\ replaced' = replaced \ {q \in Paths : q = p \/ Parent(q) = p}
    /\ UNCHANGED <<live, liveData, durableData, nextInode>>

SyncDir(p) ==
    /\ steps < MaxSteps
    /\ live[p] = Directory
    /\ SyncIntoCarried(p) \/ SyncIntoDurable(p)
    /\ steps' = steps + 1

AvailableMoves == {move \in Moves :
    /\ move.from \in Paths
    /\ move.to \in Paths
    /\ \A pair \in move.pairs : pair.old \in Paths /\ pair.new \in Paths}

Move(move) ==
    /\ steps < MaxSteps
    /\ move \in Moves
    /\ live[move.from].kind \in {"file", "dir"}
    /\ \A pair \in move.pairs : live[pair.old] # Absent <=> pair.old \in Below(move.from)
    /\ ParentLive(Parent(move.to))
    /\ \A pair \in move.pairs : Parent(pair.new) = Parent(move.to)
        \/ \E ancestor \in move.pairs : ancestor.new = Parent(pair.new)
    /\ live[move.to] \in {Absent, Directory}
    /\ live[move.to] = Directory => \A q \in DirectChildren(move.to) : live[q] = Absent
    /\ LET captured == MoveSnapshot(move.from, move)
       IN /\ live' = [p \in Paths |->
                IF \E pair \in move.pairs : pair.old = p THEN Absent
                ELSE IF \E pair \in move.pairs : pair.new = p
                THEN LET pair == CHOOSE pair \in move.pairs : pair.new = p
                     IN live[pair.old]
                ELSE live[p]]
          /\ carried' = IF live[move.from] = Directory
                       THEN [carried EXCEPT ![move.to] = captured]
                       ELSE carried
          /\ replaced' = replaced \cup {move.from, move.to}
    /\ steps' = steps + 1
    /\ UNCHANGED <<durable, liveData, durableData, nextInode>>

Crash ==
    /\ steps < MaxSteps
    /\ live' = IF Control = "crash-keeps-live" THEN live ELSE durable
    /\ liveData' = IF Control = "crash-keeps-live" THEN liveData ELSE durableData
    /\ carried' = [p \in Paths |-> NoSnapshot]
    /\ replaced' = {}
    /\ steps' = steps + 1
    /\ UNCHANGED <<durable, durableData, nextInode>>

Next ==
    \/ \E p \in Paths : CreateDir(p) \/ RemoveEmptyDir(p) \/ RemoveTree(p)
    \/ \E p \in Paths, value \in Values : CreateFile(p, value) \/ WriteFile(p, value)
    \/ \E p \in Paths : RemoveFile(p) \/ SyncFile(p) \/ SyncDir(p)
    \/ \E move \in AvailableMoves : Move(move)
    \/ Crash

Spec == Init /\ [][Next]_vars

\* Audience: internal.
LiveAndDurableTyped ==
    /\ live \in [Paths -> Node]
    /\ durable \in [Paths -> Node]

\* Audience: internal.
DataTyped ==
    /\ liveData \in [1..MaxSteps -> Values \cup {None}]
    /\ durableData \in [1..MaxSteps -> Values \cup {None}]

\* Audience: internal.
CountersTyped ==
    /\ nextInode \in 1..MaxSteps + 1
    /\ steps \in 0..MaxSteps

\* Audience: internal.
DurableDataOwned ==
    \A i \in 1..MaxSteps :
        durableData[i] = None \/ \E p \in Paths :
            durable[p].kind = "file" /\ durable[p].inode = i

TypeOK == LiveAndDurableTyped /\ DataTyped /\ CountersTyped /\ DurableDataOwned

\* Audience: internal. Every surviving live file has bytes, and a durable file keeps durable bytes.
FileDataOwned ==
    /\ \A p \in Paths : live[p].kind = "file" => liveData[live[p].inode] \in Values
    /\ \A p \in Paths : durable[p].kind = "file" => durableData[durable[p].inode] \in Values

\* Audience: internal. Parent links in both trees are directories.
TreeShape ==
    /\ \A p \in Paths : Parent(p) # 0 /\ live[p] # Absent => live[Parent(p)] = Directory
    /\ \A p \in Paths : Parent(p) # 0 /\ durable[p] # Absent => durable[Parent(p)] = Directory

Witness_DoubleNameAfterCrash ==
    ~\E a, b \in Paths :
        /\ a # b
        /\ durable[a] = durable[b]
        /\ durable[a].kind = "file"

Witness_FileReplacedByDirectory ==
    ~\E p \in Paths : live[p] = Directory /\ durable[p].kind = "file"

Witness_CarriedMove == ~\E p \in Paths : ~SnapshotEmpty(carried[p])

Witness_UnknownNameSurvivesCrash == ~\A p \in Paths : live[p] = durable[p]

=============================================================================
