# canopy-lock-holder

Cross-process fixtures for canopy writer exclusion, recovery and discovery.

**Internal crate, not published.**

## Why it exists

A canopy root permits attached writers to take turns under coordination exclusion.
The property that matters is what the **operating system** does when
the lock's owner dies without running any destructor -- and only a separate process
that is killed outright can show that. In-process unit tests over `MemFs` cannot.

The fixture has to be a real binary so Cargo exposes its path to integration tests
(`CARGO_BIN_EXE_canopy-lock-holder`). It lives in its own crate so that `canopy`
stays library-only and this binary never ships with it.

## The binary

```text
canopy-lock-holder <root>
```

`<root>` must be an initialized store whose lock is free. On success the process
prints `locked`, flushes stdout, and holds coordination exclusion until stdin closes or it is killed. Any failure is
reported on stderr with a non-zero exit, so a caller never mistakes "could not
acquire" for "acquired and waiting".

## Tests

[root_lock.rs](tests/root_lock.rs) spawns the binary over a real directory using `OsFs` and checks
that:

- a live exclusion holder permits `Store::open` but delays a write;
- killing the holder (`TerminateProcess` on Windows, `SIGKILL` on Unix) releases the
  root without any cleanup by the child;
- the lock file the dead holder leaves behind does not block a successor;
- holders of different roots do not exclude each other;
- after the holder dies, the root reopens with a clean commit log.

## The `vfs-lock-holder` binary

```text
vfs-lock-holder <path> <shared|exclusive> <file|OFFSET+LEN>
```

Takes one `Vfs::try_lock` lock over `OsFs` and holds it, with the same handshake and
exit behavior as `canopy-lock-holder`. [vfs_lock.rs](tests/vfs_lock.rs) uses it to check that a
lock held in another process excludes this one, that shared holders in two processes
coexist, that a range lock covers only its range, and that killing the holder
releases its lock.

Run the crate with the `cargo_test` MCP tool, selecting package `canopy-lock-holder`.
Linux execution uses the opt-in WSL toolchain described in
[copilot-instructions.md](../../.github/copilot-instructions.md#running-tests-on-linux-via-wsl).

## The `canopy-store-writer` binary

```text
canopy-store-writer <root> <node> <document> <body> [mode] [version]
```

The default mode writes one document, reports `opened` and `wrote`, and retains its attachment
until stdin closes. An optional version changes its software version, not its agent identity.
All successful mutations check their independent agent stamp before reporting success.

| Mode | Checkpoint or operation |
|---|---|
| `write` | An unlogged document write. |
| `pause` | Wait before the first namespace rename of an unlogged publication. |
| `pause-publication` | Wait after one namespace replica has moved in an unlogged publication. |
| `pause-before` | Wait under exclusion before the operation's first effect. |
| `pause-after` | Wait after document effects, before releasing the claim and exclusion. |
| `transaction` | A logged transaction writes the document and its `-peer`. |
| `pause-transaction` | Wait after one namespace replica has moved, after durable prepare. |
| `fail-transaction` | Fail that rename, report `failed`, and remain attached with the fence set. |
| `recover-step` | Report `phase:<phase>` and wait after each bounded recovery unit. |
| `scrub-paused` | Wait between scrub document holds so another process can resolve. |
| `session` | Read commands without automatically recovering at open. |
| `foreign` | Use acadia's test-support alternate factory and report a mismatched root as `refused`. |
| `upgrade-safe` | Parent-driven body upgrade; its default declared floor is the source. |
| `upgrade-unsafe` | Parent-driven body upgrade; its default declared floor is the target. |
| `upgrade-after-schema` | Wait at intent removal after schema publication; report `schema-advanced`. |
| `resume-upgrade` | Open with the target schema and explicitly resume the caller-driven body. |
| `reader` | Retain a read-only attachment at its declared schema. |
| `reader-wait` | Report `admission-gate` immediately before attempting its read-only gate lock. |
| `source-probe` | Attempt writer admission and report incompatible schemas as `schema-refused`. |

A paused checkpoint reports `paused` and proceeds when the parent writes a line. Recovery-step
checkpoints use the phase line instead. EOF at either checkpoint terminates the process; after
a completed operation, EOF is an orderly exit. Session commands are `write`, `transaction`,
`recover`, `resolve`, `quiesce`, `scrub`, `step` and `status`. Writes name documents with a
successive suffix and report `wrote:<sequence>`; refused writes report `recovery-required`.
`step` reports `phase:<phase>:<finished>`. `status` reports the recorded agent version.
`quiesce` acknowledges its attempt with `quiescing` before reporting `quiesced` on completion.

[shared_count.rs](tests/shared_count.rs) and [turns.rs](tests/shared_count/turns.rs) test interleaved
workloads, death boundaries, recovery interruption and concurrent recovery, log-reset exclusion,
scrub claims, agent stamps and registry entries. They execute on Windows and on Linux under WSL;
cross-compilation alone does not test what the kernel does when a process dies.

## Upgrade Commands

The schema-aware modes accept up to four numeric arguments after the software version:
`source`, `target`, `oldest-reader`, and `units`. Values are explicit test inputs; reader and
source-probe modes use `source` as their spoken schema. `resume-upgrade` uses it as the target
schema spoken by the resumer. Every body unit applies an idempotent transaction over its own
document batch, using the durable unit ordinal in the names.

An upgrade session acknowledges `opened` without automatically recovering the source. `begin`
starts conversion and reports `pending` or `intent`; `poll` retries acquisition without waiting
on locks. `step` reports `unit:<next-unit>` after durable cursor advancement. `complete` reports
`completed`, except that the after-schema mode first reports its checkpoint and waits for a line.
`fail-unit` fails after its transaction and before its cursor, reports `body-failed`, and exits
with failure. A refused upgrade reports `upgrade-refused`; admission probes report `opening`
before attempting the open and `schema-refused` on schema incompatibility.

[upgrades.rs](tests/shared_count/upgrades.rs) drives writer and reader blockers, gate-waiting
admission, competing conversions, injected body failure, and outright process death after intent,
between units, after the final unit, and after schema advancement but before intent removal.
Target-speaking resumer processes use `Recovery::step_upgrade` only in the body phase and inspect
the resulting document contents, schema and recovery state. Registry checks name live blockers
and reject killed processes as attached. These checks execute on both Windows and Linux kernels.

## Bounded Contention Campaign

[contention_bench.rs](src/bin/contention_bench.rs) runs real processes over fresh `OsFs` stores.
Build `canopy-contention-bench` with the `cargo_build` MCP tool, package `canopy-lock-holder`,
`release: true`, and `working_dir` set to the repository root. Run the optimized binary:

```powershell
target/release/canopy-contention-bench.exe .scratch/contention-windows.ndjson 0.25 10
target/release/canopy-contention-bench.exe analyze .scratch/contention-windows.ndjson
```

Arguments are raw report path, arrival window in seconds, replication count, and optional
`smoke` to select one input from each family. Reduction also writes a sibling
`.summary.ndjson` file. Reports and temporary roots live under the supplied report directory;
use the repository's `.scratch/` directory. A protocol error or failed release fails the run.

Build through `toolchain: "canopy-wsl"` for Linux, then execute the Linux binary rather than
merely cross-compiling it:

```powershell
wsl -d Ubuntu -- /tmp/canopy-linux/release/canopy-contention-bench /mnt/q/github/canopy/.scratch/contention-linux.ndjson 0.25 10
```

Adjust the mounted repository path and bridge artifact directory for the machine. This example
uses the Windows-backed WSL mount, not a native Linux filesystem; kernel and storage effects
must not be conflated when comparing platforms. Run campaigns sequentially without a competing
build or test suite. Host-time pacing, think times and process watchdogs intentionally use real
clocks; the statistical unit tests have no real-clock dependency.

The service inputs are utilization 0.3 through 0.95, with observer work at 0, 5 and 20 ms;
calibration sets arrival rates from actual logged transaction holds. Each transaction writes two
1 KiB documents with one pointer replica. A recovery perturbation selects requests with probability
0.01 and fails publication of a prepared transaction before explicit recovery; a controlled
recovery sample ensures that mechanism is exercised even when the random finite window selects none.
It measures the actual recovery shape, not CQ-1's assumed multiplier. Cooperative bursts use
4, 16, 64 and 256 members, four writes each and exponential 50 ms inter-write think times;
the short campaign conditions on one burst of the original 120-second period. Metadata writers
arrive at 1/hour, 1/minute, 1/second and 10/second beside 500 snapshot reads/second. Default and
polled modes replay identical arrival schedules. The armed-notice case measures count publication
with an acknowledged registration and requires an observed count change, not merely a timeout ring.
Upgrade arrivals use 1, 10, 100, 1000 and 2000 attachments/second and exponential lifetimes of
1, 60 and 3600 seconds, with safe writer-tier and unsafe reader-tier drains.

Raw records retain cross-process monotonic request times, acquisition generations, hold times,
publication call times, request completion and unfinished work. Wait-to-acquisition includes IPC
and local admission; repeated acquisitions during one recovery retain the originating request.
Snapshot elapsed time includes dispatch and the 1 ms read body, so it conservatively bounds the
snapshot's wait rather than equaling the simulator's wait-only metric. Shutdown cancels future burst
writes and attachment lifetimes but drains operations already issued. Upgrade censoring is sampled
before cancellation and shutdown, never replaced by a completed wait.

These are empty-start finite windows without a discarded warm-up. The reducer reports empirical
percentiles and normal-approximation intervals across replication estimates, not stationary-tail
confidence. A zero-failure bound is explicitly conditional on independent Bernoulli observations;
overlapping snapshots share writers and do not satisfy that assumption. An absent rare arrival,
an insufficient p99.9 sample or a censored minute/hour lifetime is unknown, not a budget pass.
The owned budgets and simulation comparison are in canopy's
[DESIGN-NOTES.md](../canopy/DESIGN-NOTES.md#how-long-the-contract-makes-a-caller-wait-2026-11-12).
