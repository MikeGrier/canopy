# CHECKLIST -- formal state model (TLA+) and code conformance

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Formal state model in TLA+". Discussion:
[DESIGN-SESSION-2026-11-10-formal-model.md](design-sessions/DESIGN-SESSION-2026-11-10-formal-model.md).
Completed items: [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

An item's ID is its milestone and its place there (`M6-2`, or `G-1` for a gated item); `(was ...)`
names the ID that design notes and the archive cite it by.

## Gated -- waiting on an event this checklist does not schedule

Each item here names the event it waits on; it joins a milestone once that event has happened.

- [ ] **G-1** (was `SPEC-2.4`) -- Verify the TLA+ sweep workflow on `main`, as
  [DESIGN-NOTES.md](DESIGN-NOTES.md) "The full sweep runs when main changes what it covers"
  specifies: the push that brings it to `main` starts a run of the probes and the sweep, and a push
  that changes nothing it covers starts none. Time that run on the hosted runner, and decide with
  the engineer whether the runner stays hosted or becomes self-hosted, recorded in the same
  decision. Waits on the workflow reaching `main`.

- [ ] **G-3** -- Close the deferred unreduced `SharedPublication` pairs: run
  `tools/check-shared-publication-pairs.ps1 -Tier all -Closure`, so that every positive
  configuration in the generator's `$boundaryClosure` table is closed in the unreduced model as
  well as through the view ([DESIGN-NOTES.md](DESIGN-NOTES.md) "Unreduced closures wait for a frozen
  boundary"). Before starting, estimate the cost from the latest measurements and agree it with the
  engineer. Waits on a frozen milestone or release candidate of the publication composition.

## M9+ -- Concrete Canopy schema migrations

- [ ] **CANOPY-MIGRATION** -- Specify a concrete Canopy source/target schema change and supply its
  store-owned idempotent units to the migration engine. **BLOCKER:** the repository defines Canopy
  schema 1 but no newer schema's data or layout changes. The engineer agreed on 2026-11-14 that the
  engine must reject undefined transitions rather than invent a format change. Graduate this item
  when a concrete schema change is specified; the generic engine is the current milestone's work.

## M11 -- Conformance harness

- [x] **M11-1** (was `FM-3.1`) -- Export TLC behaviours as a JSON trace corpus: every timeline's replay, random walks of the protocol as built, and the named defects' counterexamples. -> [completed 2026-11-15](COMPLETED-CHECKLIST.md#m11-1)

- [x] **M11-2** (was `FM-3.2`) -- The replay harness and `PointerPublication`'s bridge: every `PointerPublication` trace replays against `Store` on `MemFs` + `FaultyFs`, with the model refined to the store's commit-log handling. -> [completed 2026-11-15](COMPLETED-CHECKLIST.md#m11-2)

- [x] **M11-3** -- `Creation`'s bridge and the step scheduler: every `Creation` trace replays against acadia's creation contract through both implementations, with the model refined to the code's decisions. -> [completed 2026-11-15](COMPLETED-CHECKLIST.md#m11-3)

- [x] **M11-4** -- Map `Coordination`'s writer actions to the code, and agree the step hook with the engineer: the inventory, the hook's interface, the abstract write's binding, and the writer-only corpus. -> [completed 2026-11-15](COMPLETED-CHECKLIST.md#m11-4)
  > **CROSS-COMPONENT HANDOFF:** next work is in component `crates/acadia` -> M4 -> `AC-5` (the
  > shared implementation's step hook). See [COMPLETED-CHECKLIST.md](../acadia/COMPLETED-CHECKLIST.md#ac-5).

- [x] **M11-5** -- `Coordination`'s writer actions bound to the code: every writer trace replays against acadia's contract through each implementation whose behaviour it is, with the model refined to the code's recovery and crash. -> [completed 2026-11-15](COMPLETED-CHECKLIST.md#m11-5)

- [x] **M11-6** -- The SharedPublication Store replay binds document and recovery steps to both coordination implementations. -> [completed 2026-11-16](COMPLETED-CHECKLIST.md#m11-6)

- [x] **M11-7** -- Bound the remaining Coordination bridge actions and classified every control timeline. -> [completed 2026-11-20](COMPLETED-CHECKLIST.md#m11-7)

- [x] **M11-8** -- The conformance replay detects removal of collection's namespace sync at the durable-changing `CollSync` action. -> [completed 2026-11-20](COMPLETED-CHECKLIST.md#m11-8)

- [x] **M11-9** -- Establish the recovery quotient through an executable reduced/unreduced pair gate. -> [completed 2026-11-22](COMPLETED-CHECKLIST.md#m11-9)

- [x] **M11-10** -- Recovery completion rechecks claims after an intervening holder. -> [completed 2026-11-20](COMPLETED-CHECKLIST.md#m11-10)

- [x] **M11-11** -- Check the soundness of `SharedPublicationCheck`'s `QuotientView` one step at a time. -> [completed 2026-11-22](COMPLETED-CHECKLIST.md#m11-11)

- [x] **M11-12** -- Collapse unnamed blob versions in `QuotientView`. -> [completed 2026-11-22](COMPLETED-CHECKLIST.md#m11-12)

## M12 -- Extend the publication model

- [x] **M12-1** -- Extend the publication model to multi-document transaction swap sets. -> [completed 2026-11-23](COMPLETED-CHECKLIST.md#m12-1)

- [ ] **M12-2** (was `FM-4.2`) -- Extend M12-1's swap sets to node paths and node removal in one
  namespace tree. Model write/remove collisions within a swap set, removal at or beneath an existing
  document, directory creation and rename, and the directory-intent record (`.dir-sync-debt`) with
  torn appends. Reuse the directory-debt phase already in
  [SharedPublication.tla](spec/SharedPublication.tla); do not make it a second cleanup protocol.
  Bind the namespace actions in the conformance bridge and corpus, extend the quotient/soundness
  checks where removal or torn intent leaves additional state, and add witnesses for each collision
  and recovery path. **Verify:** the namespace, directory-debt and transaction filters, audience
  tags, replay corpus and the in-scope crate's tests.

### M12-2 parallel directory-soundness execution

Run this operational plan from one committed source snapshot. Do not edit the model, generator,
base configurations, generated quotient configurations, or this plan while either machine is
running. `burpy` is the publishing machine; `condor-1` is the consuming clone.

1. **Publish the frozen source checkpoint from `burpy`.** Before either machine starts TLC, finish
   and validate the source snapshot, generate the quotient inputs, commit it, and push it to origin.
   This is an execution checkpoint, not an M12 completion commit; M12 remains open until every
   proof and milestone gate passes.

   ```powershell
   Set-Location Q:\github\canopy
   pwsh -NoProfile -File tools/gen-quotient-soundness.ps1
   git --no-pager diff --check
   git add -A
   git commit -m "Checkpoint M12-2 distributed TLC execution"
   $proofCommit = git --no-pager rev-parse HEAD
   git push origin mikegrier/coord
   $proofCommit
   ```

   Record the full `$proofCommit` value. Do not begin either proof run unless `git push` succeeds.
   Both machines use that commit, not a file bundle and not independently regenerated quotient
   files. Do not move, amend, or rewrite this commit while the distributed run is active. Each
   machine needs at least 120 GB free for its own `.scratch/tlc/states`; TLC spill state is local and
   is never transferred. The M12 milestone completion commit remains a later, separate action.

2. **Reproduce the toolchain.** Both machines must report the same Java implementation and version,
   and both must use the same TLC jar. The current `burpy` proof identity is Microsoft OpenJDK
   21.0.13.1, 64-bit, using
   `C:\Program Files\Microsoft\jdk-21.0.13.102-hotspot\bin\java.exe`; `condor-1` must install or
   select that same implementation and version. Verify the jar and Java before starting:

   ```powershell
   java -version
   $jar = Join-Path $env:USERPROFILE '.tla\1.8.0\tla2tools.jar'
   (Get-FileHash -LiteralPath $jar -Algorithm SHA256).Hash
   ```

   The jar hash must be
   `fdffef247f563ef6edbfd44958d2a82db79e7564e07cb673262d896353fa6713`.

3. **Prepare `condor-1` from the pushed checkpoint.** Clone `origin` or update an existing clone,
   fetch the publishing branch, and check out the exact full commit hash recorded by `burpy`.
   Replace the placeholder with that full hash before running the commands. Do not regenerate the
   quotient files after checkout; the committed bytes are the evidence inputs.

   ```powershell
   $proofCommit = '<full commit hash copied from burpy>'
   $root = 'C:\src\canopy-m12-2'
   if (-not (Test-Path (Join-Path $root '.git'))) {
       git clone https://github.com/MikeGrier/canopy.git $root
   }
   Set-Location $root
   git fetch origin
   git checkout --detach $proofCommit
   if ((git --no-pager rev-parse HEAD) -ne $proofCommit) {
       throw 'condor-1 is not at the burpy proof commit'
   }
   pwsh -NoProfile -File tools/run-tlc-all.ps1 -Spec crates/canopy/spec/QuotientSoundness.tla `
       -Tier evidence -Filter '*DirectoryCreation-*' -List
   ```

   `HEAD` must equal `$proofCommit`. The list must contain exactly these eight evidence
   configurations: `AppendPlan`, `AppendRepair`, `AppendSync`, `Complete`, `Create`, `Recovery`,
   `Sync`, and `Transaction`. If either check differs, stop; `condor-1` is not using the same
   frozen source snapshot as `burpy`.

4. **Run the largest closure on `burpy`.** After both machines have passed the commit, Java, jar,
   and configuration checks, start this command at the agreed common start time. Use one 24 GB TLC
   heap and eight workers. Direct the runner's summary and impact record to `burpy`-specific files
   so nothing can be confused with the other machine.

   ```powershell
   Set-Location Q:\github\canopy
   $env:Path = 'C:\Program Files\Microsoft\jdk-21.0.13.102-hotspot\bin;' + $env:Path
   $record = '.scratch/tlc/record/m12-2-burpy.json'
   $summary = '.scratch/tlc/m12-2-burpy-terminal.out'
   pwsh -NoProfile -File tools/run-tlc-all.ps1 `
       -Spec crates/canopy/spec/QuotientSoundness.tla `
       -Tier evidence `
       -Filter '*DirectoryCreation-Transaction*' `
       -Workers 8 -HeapGB 24 -Record $record 2>&1 | Tee-Object -FilePath $summary
   if ($LASTEXITCODE -ne 0) { throw "burpy directory proof failed with exit $LASTEXITCODE" }
   ```

5. **Run the seven smaller closures on `condor-1`.** Start this at the same agreed time as `burpy`.
   Run at most three configurations at once, each with an 8 GB heap and four workers. This uses
   roughly 12 TLC workers and 24 GB heap aggregate. The runner advances to the remaining
   configurations as slots become free.

   ```powershell
   Set-Location C:\src\canopy-m12-2
   $env:Path = 'C:\Program Files\Microsoft\jdk-21.0.13.102-hotspot\bin;' + $env:Path
   $record = '.scratch/tlc/record/m12-2-condor-1.json'
   $summary = '.scratch/tlc/m12-2-condor-1-terminal.out'
   pwsh -NoProfile -File tools/run-tlc-all.ps1 `
       -Spec crates/canopy/spec/QuotientSoundness.tla `
       -Tier evidence `
       -Filter '*DirectoryCreation-AppendPlan*,*DirectoryCreation-AppendRepair*,*DirectoryCreation-AppendSync*,*DirectoryCreation-Complete*,*DirectoryCreation-Create*,*DirectoryCreation-Recovery*,*DirectoryCreation-Sync*' `
       -Parallel 3 -Workers 4 -HeapGB 8 -Record $record 2>&1 | Tee-Object -FilePath $summary
   if ($LASTEXITCODE -ne 0) { throw "condor-1 directory proof failed with exit $LASTEXITCODE" }
   ```

6. **Return only results from `condor-1`.** Package the seven named TLC output files, the summary,
   and the `condor-1` record; do not package `.scratch/tlc/states`.

   ```powershell
   $names = 'AppendPlan','AppendRepair','AppendSync','Complete','Create','Recovery','Sync'
   $results = @('.scratch/tlc/m12-2-condor-1-terminal.out',
                '.scratch/tlc/record/m12-2-condor-1.json')
   $results += foreach ($name in $names) {
       ".scratch/tlc/QuotientSoundness-DirectoryCreation-$name.out"
   }
   Compress-Archive -Path $results -DestinationPath .scratch/m12-2-condor-1-results.zip
   Get-FileHash -LiteralPath .scratch/m12-2-condor-1-results.zip -Algorithm SHA256
   ```

   Transfer that archive to `burpy` and record its SHA-256. The result archive should be far below
   1 MB unless TLC emitted a counterexample trace.

7. **Reconcile on `burpy`.** Extract the `condor-1` results under a separate directory so its record
   cannot overwrite the `burpy` record. Each machine's terminal summary must report
   `mismatched: 0`: `burpy` reports `checked: 1`; `condor-1` reports `checked: 7`. Each of the eight
   configuration logs must end with `Model checking completed. No error has been found.` Missing,
   interrupted, stale, deadlocked, invariant-violating, or out-of-memory output is not a pass.
   Preserve `$proofCommit`, both result hashes, both terminal summaries, both impact records, and
   all eight `.out` files as the M12-2 evidence set. After every other M12 item and the required
   milestone gates pass, create the M12 completion commit and push the resulting branch to origin.

- [ ] **M12-3** (was `FM-4.3`) -- Add repair and scrub to the publication composition. Model the
  writer exclusion and modification count a repair holds odd, the live and durable replica checks,
  repair of a version another pointer or record still names, report-driven repair, and a scrub's
  fault and retry behavior. State how each action interacts with an active holder and recovery, and
  prove that repair never makes an unnamed blob version nameable. Extend the quotient view and
  one-step perturbations, add discriminating controls and witnesses, and bind the new actions to the
  Store conformance bridge and trace corpus. **Verify:** the repair, scrub, publication and soundness
  filters, audience tags, replay corpus and the in-scope crate's tests.

## M14+ -- TLA futures

Work on how the models are checked that waits until the milestones above are done.

- [ ] **M14+-1** (was `SPEC-3`) -- Generate mutants of a spec mechanically, to find the rules no property checks.
  The probe corpus under `spec/probes/` is written by hand, one mutant per rule someone named, so
  it shows whether each named rule's property can fail and says nothing about a rule nobody
  named. Build in `tools/` a generator that applies a small set of operators to every action of a
  spec -- drop a conjunct, negate a guard, leave a primed variable unchanged, widen a set
  membership -- and runs each mutant against the configurations that reach the action, the way
  `cargo mutants` is run over code. A mutant no configuration catches is triaged into a new
  property, a new scenario or bound that exposes it, or a recorded equivalent, and the triage is
  what the pass is for. Deferred by the engineer on 2026-11-11 until the milestones above are
  done: building the generator is substantial work, and a full pass is expected to take days of
  TLC time, which the current work cannot spare. When it is taken up, its runs belong with
  SPEC-2's scheduled full run, scoped by SPEC-2's impact analysis.

  Verify: a spec edit that removes a guard checked by no property yields a surviving mutant, and
  one checked by a property does not.
