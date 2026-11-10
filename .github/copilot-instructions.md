# Copilot Instructions

Use LF line endings.

## PRIME DIRECTIVE — never defer work for a perceived lack of need

**Do not defer, drop, or narrow a feature merely because you cannot currently see a
consumer/client for it.** "Nothing calls this yet," "no client needs it now," "YAGNI,"
and "we'll add it when a caller appears" are **not** acceptable reasons to defer work
that the design or the task calls for. Absence of a visible client is not a blocker — it
is just absence of information, and it must never be used as a justification to shrink
scope.

The **only** legitimate reason to defer a piece of work is a genuine **blocking factor** —
a real dependency that does not yet exist, a hard technical impossibility, an unresolved
design decision, an external constraint, or a resource/tooling gap. When you hit a genuine
blocker:

1. **Name the blocker explicitly** — state precisely what is blocking the work and why.
2. **Raise it with the user** and discuss how to work *around* the block (build the missing
   dependency, sequence differently, find an alternative approach) rather than silently
   parking the feature.
3. **Only defer if the user agrees** the block is real and cannot be worked around now — and
   record the specific blocker (not "no client") as the deferral reason.

When in doubt, **implement it now** and surface the decision, rather than deferring and
moving on. If you find yourself writing a deferral note whose rationale is "no consumer
yet," "not needed for this witness," or any variant of "we don't need it," **stop** — that
is exactly the anti-pattern this directive forbids. Convert it into either (a) implementing
the work, or (b) an explicit blocker conversation with the user.

## PLATFORM INTEGRITY — layers and platforms are isolation boundaries, never collapsed for convenience

**This repository is built as layers and platforms on purpose. Do not omit, tune,
merge, delete, or "optimize" a feature in a way that subverts the intent of that
layering.** Treating a layer boundary as an inefficiency to be collapsed — folding two
paths into one, dropping a lower baseline to serve a higher one, skipping a platform's
feature because the currently-visible goal does not exercise it — is a **recurring,
costly anti-pattern** that undoes deliberate design and delays the project. When you
see what looks like a "layering inefficiency," **assume it is deliberate isolation and
ask about intent before proposing to consolidate it.** An assistant's efficiency
instinct is never a licence to redesign a plan the engineer built deliberately; help
coax the plan out and execute it, do not subvert it.

Three binding rules follow:

1. **Duplicate-then-decide is standard procedure for speculative work.** Building a new
   capability as a *separate, duplicated* path alongside the existing one — so the
   speculative work proceeds *without interfering with or disrupting* the working path —
   is correct and expected, **especially** for a feature whose dependency is not yet
   certain. The duplication is **not** debt to be minimized; it is the mechanism that
   keeps the working layer stable while the new one is proven. The **merge-or-delete**
   decision is made **when the new path is proven**, never pre-empted mid-development,
   and never traded away for a testing/efficiency shortcut that re-couples the paths.
   Track the *when-done* merge-or-delete decision so it is not forgotten — a duplicated
   path silently becoming permanent because nobody circled back is the real failure
   mode, not the duplication.

2. **Depend on specified primitives, never on incidental current behavior.** A consumer
   must bind to a layer's *specified* contract — its primitives, its documented
   semantics — never to how the layer happens to behave right now. Leaning on an
   implementation's incidental behavior (e.g. relying on how a code generator currently
   emits code plus a specific CPU's memory-ordering side effects, instead of using the
   specified atomic / memory-ordering primitives) is a correctness trap that has already
   cost days of preventable waste. This is the consumer-side twin of "Design Autonomy —
   Behavior is owned, never inherited from dependencies" below: each layer owns its
   behavior, and each consumer binds only to that owned specification.

3. **Do not narrow the platform to serve the visible goal.** Every platform component
   must remain a *level* platform — its lower baselines and less-exercised features are
   first-class, not optional trimmings to cut because the current task does not need
   them. This is the PRIME DIRECTIVE above viewed from the platform side: absence of a
   visible consumer for a layer or feature is never a licence to omit, downgrade, or
   collapse it.

When a genuine constraint (a real cost, a real blocker) makes a layer duplication or a
baseline truly unaffordable, that is a **decision for the engineer driving the work**,
raised explicitly per the PRIME DIRECTIVE's blocker protocol — never a shortcut an
assistant takes unilaterally in the name of efficiency.

## Line endings in tool parameters

All text content passed to tpu tools (`content`, `replacement`, `data` in edit ops) is
automatically normalized to LF before processing. You do not need to worry about whether
the text you send uses LF or CRLF — tpu handles the conversion. The file's existing
line-ending convention (LF or CRLF) is preserved on disk automatically.

**Creating a new file: use `tpu_write_file`, never the built-in `create_file`.**
This repo is LF-only. The built-in `create_file` writes **CRLF** on Windows even when
the `content` you pass uses LF, and git then rejects the commit with
`fatal: CRLF would be replaced by LF`. `tpu_write_file` writes LF by default for a new
file (verified: identical content is 17 bytes via `tpu_write_file` vs 20 bytes via
`create_file`). The same "prefer `tpu_*` over the built-in file tools" rule applies to
edits (`tpu_replace_in_file` / `tpu_edit_file`), not only to PowerShell/shell.

## Terminal / Git rules — hang prevention

**These rules prevent terminal hangs that freeze the session.**

- Every `git` command that can produce paged output **must** be run with
  `git --no-pager <subcommand>`. The list below is illustrative, not exhaustive:
  `diff`, `show`, `log`, `blame`, `reflog`, `stash list`, `branch -v`,
  `shortlog`, `tag -n`, `whatchanged`, `grep`. **If unsure whether a `git`
  subcommand may page, use `--no-pager`.**
- Never run `git commit` without `-m "…"`. Commit messages must be a **single
  line** when supplied via `-m`. For longer messages write the message to a
  file under `.scratch/` and use `git commit -F .scratch/<file>`. Never use
  PowerShell here-strings (`@"…"@`) or embedded newlines inside `-m` —
  PowerShell will either hang waiting for terminator or pass `\n` literally.
- **Rust pre-commit gate:** If any staged file has a `.rs` extension, you
  **must** run `cargo fmt` then `cargo clippy --all-targets` (via the
  `mcp_cargo-mcp_cargo_fmt` and `mcp_cargo-mcp_cargo_clippy` tools) and fix
  all issues before running `git commit`. Any issue reported by either tool
  must be resolved before continuing — do not proceed to `git commit` while
  any formatting diff or Clippy diagnostic remains. You **must** also verify
  the staged diff adds no inline `#[cfg(test)] mod tests { ... }` block
  (`git --no-pager diff --cached -U0 -- "*.rs" | Select-String '^\+\s*mod tests\s*\{'`);
  any hit is a blocking violation — move those tests into a sibling `tests.rs`
  first.
- **Commit every file `cargo fmt` reformats, even outside your task's scope.**
  `cargo fmt` rewrites *all* files in the formatted scope, not just the ones you
  edited — so a run can clean up a pre-existing formatting drift in a file your
  current change never touched. When that happens you **must** stage and commit
  that reformatting too (in the same commit, or a separate `Format <file> (cargo
  fmt)` commit if you want to keep it isolated). **Never** leave a
  `cargo fmt`-produced diff unstaged on the grounds that the file is "out of
  scope": doing so leaves the working tree dirty, silently defers the cleanup,
  and lets the next contributor mistake it for their own change. If a fmt run
  touches files you did not expect, that is expected behavior — commit them, do
  not `git checkout` them away.
- Never run `git pull` or `git merge` without `--no-edit`. **`--no-edit` is a
  `merge`/`pull` flag only** — `git rebase` does **not** accept `--no-edit` (passing
  it makes `git rebase` fail and print its usage). A plain, non-interactive
  `git rebase <upstream>` opens no editor on its own, so it needs no such flag: run
  it bare (never add `--no-edit`, and never use `-i`).
- Never run interactive commands: `git rebase -i`, `git add -p`, etc.
- Do not use `less`, `more`, or any other interactive pager.
- Never use PowerShell multi-line string operators (`@"…"@`) in terminal commands.

## Cargo commands — use cargo-mcp tools, never the terminal

Always use the `cargo_*` MCP tools instead of running `cargo` commands in a terminal.
This applies even inside a larger workflow — do not switch to the terminal for cargo
just because a previous step used the terminal.

**Always set `working_dir` to the workspace root** (the absolute path of the
repository root, which holds the workspace `Cargo.toml`). Every
cargo-running tool accepts a `working_dir` parameter. If you omit it, the call
defaults to the cargo-mcp server's own working directory (typically the user's
home folder), which is almost never this project — the call then fails to find a
`Cargo.toml`/toolchain, or operates on the wrong manifest. Pass the absolute path
of the workspace root on every call. The server rejects a `working_dir` from which
no `Cargo.toml` can be found by walking upward, returning an actionable error
instead of running in the wrong place.

| MCP tool | Replaces |
|---|---|
| `cargo_metadata` | `cargo metadata` |
| `cargo_check` | `cargo check` |
| `cargo_build` | `cargo build` |
| `cargo_test` | `cargo test` |
| `cargo_clippy` | `cargo clippy` |
| `cargo_fmt_check` | `cargo fmt --check` |
| `cargo_fmt` | `cargo fmt` |
| `cargo_tree` | `cargo tree` |
| `cargo_doc` | `cargo doc` |
| `cargo_clean` | `cargo clean` |
| `cargo_update` | `cargo update` |
| `cargo_fix` | `cargo fix` |
| `cargo_add` | `cargo add` |
| `cargo_remove` | `cargo remove` |
| `cargo_publish` | `cargo publish` |
| `cargo_nextest_run` | `cargo nextest run` (requires cargo-nextest) |
| `cargo_nextest_list` | `cargo nextest list` (requires cargo-nextest) |
| `cargo_setup` | *(no terminal equivalent)* |
| `cargo_diagnostic` | *(no terminal equivalent)* |

### Boolean arguments

Boolean flags (`all_targets`, `release`, `workspace`, `lib`, `tests`,
`all_features`, `no_default_features`, `locked`, `offline`, …) expect a JSON
boolean (`true`/`false`). The server also accepts loose forms (`"true"`,
`"1"`, `"yes"`, `"on"` and their negatives, case-insensitive, plus the
integers `0`/`1`), but prefer the native boolean. The same coercion applies
to nested boolean fields such as `cargo_test`'s `test_filter.include_ignored`.

An unrecognised value (e.g. `"maybe"`, an object, an integer other than
`0`/`1`) is treated as `false` and the server emits a `warning` MCP
notification naming the field. **If a flag you expected (`--all-targets`,
`--release`, …) is missing from the echoed `x-cargo-mcp-invocation` argv,
look for that warning — you almost certainly sent the boolean in an
unrecognised shape.**

### Test selection — which knobs belong to which tool

`cargo_test` and `cargo_nextest_run` have **separate, non-interchangeable**
selection parameters. The server **rejects unknown arguments** with an
actionable error (including a "did you mean" hint), so a mismatched knob fails
fast instead of silently running the entire suite.

- **`cargo_test` only:** `test_name` (substring, or `exact: true`),
  `test_filter` (`{ "pattern": "<regex>", "include_ignored": <bool> }`),
  `per_test_timeout_secs` (only meaningful with `test_filter`), and
  `doc: true` (doctests — nextest cannot run these).
- **`cargo_nextest_run` only:** `filter` (substring over test names) and
  `filter_expr` (the nextest `-E '<expr>'` filterset DSL).

For a workspace with many tests across many binaries, a full run can exceed any
reasonable wall-clock cap. Narrow it with the correct tool's filter, or run
intentionally unbounded with `timeout_secs: 0`.

### cargo_test — phases & timeouts

Every `cargo_test` and `cargo_nextest_run` call runs in **two independently
timed phases**: a **build** phase (`cargo test --no-run`) followed by a **test
execution** phase. `timeout_secs` applies to each phase on its own clock, so build
time is never counted against the execution budget. A `TimeoutError` names the
phase that fired ("... during the build phase" vs "... during the test execution
phase"), so a slow compile is distinguishable from a hung test.

**Doctests are the exception.** Cargo rejects `cargo test --doc --no-run`, so
`doc: true` runs as a single phase on one clock (labelled `doc test` in a
`TimeoutError`). `no_run`, `test_filter`, and `bisect` are all rejected together
with `doc: true`.

**`timeout_secs` — hard overall wall-clock cap**, the same meaning in every mode,
applied to each phase independently.
- Unfiltered runs: server default from `cargo-mcp.test.timeoutSecs` (**30 s** via
  the VS Code extension; none otherwise).
- Filter mode (`test_filter` set): **no default** — omit to let a long matched
  run complete; pass a value to cap it.
- Pass `timeout_secs: 0` to disable for this call regardless of the server
  default.

**`test_timeout_secs` — execution-phase-only override (unfiltered mode).**
Overrides the execution phase's budget while `timeout_secs` still governs the
build. Rejected together with `doc: true`, `test_filter`, or `bisect` (each has its
own timeout model).
- If `timeout_secs` is omitted, the build phase is left **unbounded** — useful on
  a cold cache or a crate with a known-slow build.
- If `timeout_secs` is also set, `test_timeout_secs` is clamped to it: it can only
  tighten the execution budget, never loosen it.
- `test_timeout_secs: 0` is the same as leaving it unset.

**`per_test_timeout_secs` — per-test budget (filter mode only).** Ignored when
`test_filter` is not set; its clock arms on cargo's `build-finished` record, so a
slow build never trips it. Its semantics depend on the
`cargo-mcp.test.perTestExecution` setting:
- **Batched mode** (default): an idle watchdog. The clock resets on every
  `test … ok|FAILED|ignored` line, so a long suite of fast tests is fine but a
  single hung test (which stops emitting output) trips it; the offending binary's
  cargo process is killed and the orchestrator moves on. The hung test is not
  named — the last `x-cargo-mcp-test-output` line before the gap is the closest
  indicator.
- **Per-test mode** (`perTestExecution` on): a simple wall-clock cap per
  invocation. Each test runs in its own cargo process, so the timed-out test is
  named unambiguously in the `x-cargo-mcp-invocation` `argv` (`--exact <name>`).

Default is the server setting; when that is absent or `0`, filter mode still
applies a hard-coded **30 s** fallback so hung-test protection is always on. Pass
`per_test_timeout_secs: 0` to fully disable for a call.

Raise (or `0`) any of these knobs for runs you know are slow (long integration
suites, a single test that sleeps/polls); lower one when sanity-checking a fix and
you want fast feedback if a change spun into an infinite loop.

### cargo_test — per-test execution mode

When the `cargo-mcp.test.perTestExecution` VS Code setting is enabled, each
matched test in `test_filter` mode runs as its own `cargo test -- --exact <name>`
invocation (one cargo process per test). Benefits: the hung/timed-out test is
named unambiguously (in the invocation `argv`), `per_test_timeout_secs` becomes a
plain wall-clock cap rather than an idle watchdog, and tests run serially so there
is no ambiguity about which test was executing. Cost: one cargo launch per matched
test (~200–500 ms each) — fine for targeted re-runs, but use batched mode for
broad filter runs.

### Per-call environment variables (`env`)

Every `cargo_*` tool that spawns cargo accepts an optional `env` object that
sets or unsets environment variables on the cargo subprocess for that one
call. Keys are env var names; values are a string (set) or `null` (unset).
The map layers on top of cargo-mcp's defaults (`CARGO_TERM_COLOR`,
`NO_COLOR`, `RUSTC`), so a caller-supplied value wins.

Use this — never a terminal — to apply a one-shot debug knob such as
`RUSTFLAGS`, `RUST_LOG`, `RUST_BACKTRACE`, `RUSTC_BOOTSTRAP`, or a
compiler-internal dump such as a crate-specific `*_DUMP_*` variable:

```json
{ "env": { "RUST_LOG": "canopy=debug", "RUST_BACKTRACE": "1" } }
```

Do **not** use `env` for permanent/project-wide config (put that in
`Cargo.toml`, `.cargo/config.toml`, or `rust-toolchain.toml`) or for
secrets (the block is visible via OS process inspection and may be captured by
future logging).

### Redirecting full output to a file (`output_path`)

`cargo_check`, `cargo_build`, `cargo_test`, `cargo_clippy`, and `cargo_doc`
accept an optional `output_path`: a relative path (under the working
directory; no `..`; parent must already exist) that receives the **complete**
NDJSON output. When set, the tool result is a compact SUMMARY (invocation
header, an `x-cargo-mcp-output-file` pointer, all `level: error` messages,
`build-finished`, stderr, status trailer, and — for `cargo_test` — libtest
summary/failure markers); warnings, passing-test lines, artifact records, and
captured `println!` replays are dropped from the summary but preserved in the
file.

Use `output_path` when the full transcript would bloat context (long
`cargo_test` runs, large workspaces) instead of piping to a temp file. Per
the scratch-directory rule below, target `.scratch/` (e.g.
`".scratch/test-run.ndjson"`). Read the summary first; if `exit_code` is
non-zero or failure markers appear, open the file for the full transcript.

### Hang / slow-test bisection (`bisect`)

`cargo_test` and `cargo_nextest_run` accept an optional `bisect` object that
switches the call into a bisection engine for **finding which test hangs or runs
long**. It builds once, enumerates every test, then runs groups of tests under a
short kill-deadline (always single-threaded). Any group that times out (hangs) or
exceeds the slow threshold is recursively subdivided until the culprit test(s)
are isolated. It works identically on both tools (it runs the compiled libtest
binaries directly).

Only `group_timeout_secs` is required:

```json
{ "bisect": { "group_timeout_secs": 10 } }
```

Knobs (all optional except `group_timeout_secs`):

- `group_timeout_secs` (REQUIRED) — per-group kill-deadline in seconds; a group
  that exceeds it is treated as hung.
- `slow_threshold_secs` — must be < `group_timeout_secs`; a group that finishes
  but takes longer than this is `slow` and gets subdivided. Omit to detect hangs
  only.
- `split_factor` (default 2) — sub-groups per subdivision (binary search).
- `split_percent` — alternative to `split_factor`: yields ceil(100/p) sub-groups
  (mutually exclusive with `split_factor`).
- `min_group_size` (default 1) — stop subdividing at this size and report the
  members as culprits.
- `initial_group_size` / `initial_groups` — shape the first-level groups
  (mutually exclusive); default is one group of all tests.
- `max_rounds` (default 32) — cap on subdivision depth.
- `pattern` — RE2 regex; only matching `module::path::test_name` tests
  participate. `include_ignored` — also bisect `#[ignore]` tests.

The result is an NDJSON stream of `x-cargo-mcp-bisect-config`,
`x-cargo-mcp-bisect-group`, `x-cargo-mcp-bisect-culprit`, and
`x-cargo-mcp-bisect-summary` records; `output_path` is honoured (full body to
file, compact summary inline). The call is an error when any culprit is found.

### Reading cargo_test output

`cargo_test` returns a strict NDJSON stream. Parse it line-by-line; every
non-blank line is a JSON object. The `reason` field identifies the record type:

| `reason` | Content | Key fields |
|---|---|---|
| `x-cargo-mcp-invocation` | Effective command and working dir (first line) | `argv`, `cwd` |
| `compiler-message` | Compilation error or warning | `message` (rustc diagnostic) |
| `build-finished` | Build phase outcome | `success` (bool) |
| `x-cargo-mcp-test-output` | One line of libtest harness output or captured `println!` | `text` |
| `x-cargo-mcp-stderr` | `eprintln!` and other test stderr (when non-empty) | `text` |
| *(last line)* | Exit status | `status` (`"success"` or `"error"`), `exit_code` (on error) |

`println!` inside tests is captured by libtest and replayed as
`x-cargo-mcp-test-output` lines only when the test fails (standard libtest
behaviour). `eprintln!` bypasses libtest capture and always appears in
`x-cargo-mcp-stderr`.

### Optional: cargo-nextest

`cargo-nextest` is available, but this workspace has no `.config/nextest.toml`, so
`cargo_test` remains the canonical test tool. Prefer `cargo_nextest_run` when the
user asks for nextest or a run benefits from per-test process isolation, built-in
retries, or filterset expressions. `cargo_test` is the **only** way to run
doctests (nextest does not run them). `cargo_nextest_list` enumerates tests as
structured JSON when you need discovery without execution.

## Rust toolchain: `rustup`, pinned to `1.98`

This workspace pins its Rust toolchain via `rust-toolchain.toml` (`channel = "1.98"`),
installed and managed through ordinary `rustup`. No internal distribution, no
multiplexer, and no special `PATH` handling is needed: `rustup toolchain list`, `rustup
show`, and `rustup target add <triple>` all operate on it directly, the same as any other
toolchain `rustup` manages.

### Cross-compiling

```powershell
$env:CARGO_INCREMENTAL = '0'
cargo check -p <crate> --all-targets --target x86_64-unknown-linux-gnu
```

`CARGO_INCREMENTAL=0` avoids `error finalizing incremental compilation session
directory ... Access is denied. (os error 5)` on cross-target builds (a Windows
file-locking issue, unrelated to which toolchain manager is in use).

### Running tests on Linux via WSL

There is a WSL2 Ubuntu distribution with a native `cargo`/`rustc` and `gcc`, which is how
Linux-only paths get **executed** rather than merely cross-checked.

```powershell
wsl -d Ubuntu -- bash -lc "cd /mnt/<drive>/<path-to-repo>; CARGO_HOME=/mnt/c/Users/<user>/.cargo CARGO_TARGET_DIR=/tmp/canopy-linux cargo test -p <crate> --offline"
```

- WSL's own `rustup` must have the `1.98` toolchain installed to resolve
  `rust-toolchain.toml` (`rustup toolchain install 1.98` there if it is missing).
- `CARGO_HOME` pointed at the Windows cargo home plus `--offline` reuses the already
  populated registry cache instead of downloading every dependency again inside WSL.
- A separate `CARGO_TARGET_DIR` keeps the Linux build from clobbering the Windows one
  (and `/tmp` is far faster than `/mnt`).

A cross-*check* proves a platform's code compiles. It proves nothing about behavior --
anything whose value is what the OS does at runtime (locking, process death, signals)
must actually run here.

Every crate in this workspace builds and runs under WSL. That is how the POSIX `flock` path
of `acadia`'s exclusive lock, and the `canopy-lock-holder` cross-process lock tests, are
exercised on a real Linux kernel.

### Restoring a file after a revert-probe must bump its mtime

A revert-probe (deliberately neuter the implementation, confirm the tests fail, restore)
is only sound if the restore actually causes a rebuild. On Windows, `Copy-Item` **preserves
the source's `LastWriteTime`**, so restoring from a `.orig` backup written *before* the
probe leaves the file looking **older than the artifacts built from the probed source**.
Cargo then considers the crate up to date and silently reuses the **probe's** binary.

The failure is nasty because it looks like a real regression: the next `cargo test` run
reports exactly the tests the probe was designed to break, against a working tree that is
provably correct. This cost a confusing round trip while implementing `EI-2`.

It only bites when nothing else touches that file between the restore and the next build --
if the next step edits the same file, the mtime bump hides the problem, which is why it can
go unnoticed for several probes in a row.

After restoring, touch the file:

```powershell
Copy-Item "$f.orig" $f -Force
(Get-Item $f).LastWriteTime = Get-Date
```

Writing the original text back with `tpu_write_file` (or any normal edit) is equally safe,
since that sets a current timestamp. The hazard is specific to timestamp-preserving copies.

### `cargo clean --target` is not narrow

`cargo clean --target <triple>` removed the **entire** `target/` tree in this workspace
(78,044 files, 32.8 GiB), not just that target's subdirectory. Expect a full cold rebuild
afterwards, and prefer deleting the specific subdirectory if that is what you meant.

## TLA+ / TLC

Protocol specs live in a crate's `spec/` directory as `<Name>.tla` with a sibling
`<Name>.cfg` (see [crates/canopy/CHECKLIST.md](../crates/canopy/CHECKLIST.md), `FM-*`).

**Pinned toolchain** (Windows host):

- JDK: Microsoft Build of OpenJDK 21 -- `winget install --id Microsoft.OpenJDK.21 -e`.
  Open a new terminal afterwards so `java` is on `PATH`.
- `tla2tools.jar` **v1.8.0**, kept outside the repository at
  `%USERPROFILE%\.tla\1.8.0\tla2tools.jar`:

  ```powershell
  $dir = Join-Path $env:USERPROFILE '.tla\1.8.0'
  New-Item -ItemType Directory -Force -Path $dir | Out-Null
  Invoke-WebRequest -Uri 'https://github.com/tlaplus/tlaplus/releases/download/v1.8.0/tla2tools.jar' -OutFile (Join-Path $dir 'tla2tools.jar') -UseBasicParsing
  (Get-FileHash (Join-Path $dir 'tla2tools.jar') -Algorithm SHA256).Hash
  ```

  The SHA-256 must be
  `e1f11b75c4b5bc4478ba0ac5cb6ae28e40a01b0016bc33e0aa97da22dfdbcd7f` (GitHub's published
  digest for the release asset). Changing the pinned version is a deliberate change to this
  section, not a silent upgrade.

**Run a spec** with [tools/run-tlc.ps1](../tools/run-tlc.ps1):

```powershell
pwsh tools/run-tlc.ps1 -Spec crates/canopy/spec/<Name>.tla
```

It uses the sibling `.cfg` (or `-Config`), writes TLC's output to `.scratch/tlc/<name>.out`
and its state files under `.scratch/tlc/states`, suppresses TLC's generated trace-explorer
spec so nothing is written beside the spec, and exits with TLC's code: `0` when every
invariant and property holds, non-zero otherwise (`12` for a safety violation). A spec check
is therefore a gate like `cargo test`.

WSL has no JDK installed, so TLC runs on the Windows host only.

## Scratch directory for temporary files

When you need to capture command output, test results, debug logs, build warnings, or any
other temporary/diagnostic data to a file, **always write it under the `.scratch/` directory**
at the repository root. This directory is git-ignored.

- Create `.scratch/` if it does not exist.
- Use descriptive filenames (e.g., `.scratch/test_parser_output.txt`, `.scratch/build_warnings.txt`).
- **Never** write scratch or debug files to the repository root or any source directory.

## General instructions for this repository
- All code is Copyright Michael Grier.
- All source code should include a copyright statement. The statement should be brief, a single line comment as the first line of the file which reads something like: Copyright (c) Michael Grier.
- If the source file is also part of an open source library, there may be additional lines giving the details, but in general, open source content should not be checked in to this source repository except as part of a patching process to provide a patch over defective open source dependencies which have to be addressed for security or business continuity reasons.

## Engineering priorities

These are the maintainer's standing priorities. They govern design, implementation, and
code-review decisions across this repository.

1. **Correctness and reliability come first -- non-negotiable.** Never trade correctness or
   reliability for speed. If a thing does not have to work right, it can be made as fast as
   you like; but when the two conflict, correctness and reliability win by default.
2. **Design APIs that are safe by construction.** Prefer interfaces that are easy to use
   safely and that guide their consumers toward safe patterns of use -- make misuse hard and
   make the safe path the path of least resistance.
3. **Treat performance as resource "good citizenship."** Performance matters, but chiefly in
   the realm of efficiency rather than raw algorithmic speed: as the host comes under load,
   (a) do not place undue burden on it, (b) extract the best performance available from it,
   and (c) avoid large or unpredictable variation in performance. Favor eliminating needless
   work (allocations, copies), tolerating contention gracefully, and keeping latency
   predictable.

## Interaction Guidelines
- Prefer concise responses: minimize verbosity, reduce repetition, and avoid excessive formatting/emojis. Get straight to the point in all interactions.

## Checklist execution discipline

When executing checklist items (CHECKLIST.md files):

- **Implement one item at a time, in order.** Finish item N -- implemented, tested, checked
  off -- before you start reading, planning, or editing for item N+1. This is a sequencing
  rule: it stops item N+1's concerns from leaking into item N while item N is still being
  written. It says nothing about commits.
- **No reaching ahead for convenience.** Do not start item N+1 early because the work is
  similar, the context is loaded, or doing both at once feels efficient. Convenience is not a
  reason to pull future work into the current item.
- **A checklist item may legitimately be large.** There is no upper bound on an item's diff
  size, file count, or scope. If an item's work is genuinely coupled -- for example, a schema
  change that must update several layers and their tests to compile at all -- do it as one
  item. Do **not** invent sub-items (`M1.1.1`, `M1.1.2`, ...) to make it feel smaller.
- **If items are mis-structured, say so; do not paper over it.** If two items turn out not to
  be implementable independently (one cannot compile or pass tests without the other), that
  is a checklist-structuring defect. Name it, then either merge the items or restructure the
  checklist before continuing.
- **Re-plan when execution reveals planning was wrong.** A checklist is a hopeful projection,
  not a contract. When execution shows items are coupled, an item decomposes into unforeseen
  work, an item is already done, or an item's scope has changed, **stop and update the
  checklist before continuing.** Restructuring mid-execution is normal; silently working
  around a wrong plan is not.
- **Check the item off** in CHECKLIST.md (change `- [ ]` to `- [x]`) when it is done.
- **Tests must pass** before an item is checked off. Run the appropriate test command (per
  the language-specific instructions) after each item and fix failures first. Pre-existing
  failures unrelated to the current item do not block it, but must be recorded in
  `UNRESOLVED-TEST-FAILURES.md`. When such a failure is later **resolved**, do not delete its
  entry -- move it out of `UNRESOLVED-TEST-FAILURES.md` into a sibling
  `RESOLVED-TEST-FAILURES.md` (append-only) under a
  `## Resolved <YYYY-MM-DD HH:MM:SS +hh:mm> -- <description>` heading recording the date and
  time the resolution was finalized, in the same change that removes it from the unresolved
  file.
- **Commit at milestone boundaries, not per item.** When a milestone's items are all done
  and the implicit end-of-milestone steps pass (see "CHECKLIST file hygiene"), commit the
  milestone's work as **one** commit. Do not split finished work into per-item commits after
  the fact. Commit earlier only when the engineer asks.
- **Commit message format:** a one-line subject naming the milestone, then one
  `Completed item:` line per item the commit completes, e.g.:
  ```
  Completed milestone: M2 -- Test-support robustness

  Completed item: RF-6: Detect renamed and table-form test-timers dependencies
  Completed item: RF-7: Run the interval test on a paused clock
  ```
- **Rebase between work items.** After each item (and at the start of a work session), run
  `git fetch` then `git rebase --autostash <upstream>`, so drift is resolved in small steps
  rather than one large conflict at the milestone. `--autostash` carries the uncommitted
  milestone work across the rebase, so no per-item commit is needed. `git add` new files
  first -- the autostash covers tracked files only. If reapplying the stash conflicts,
  resolve the markers and `git stash drop` the leftover entry. Push at milestone
  boundaries; outside them, ask before pushing.
- **When the last item in a CHECKLIST file is completed**, update its PLANS.md entry to
  "completed" in the same commit.
- **Cross-component handoff callouts.** When the next required action in a checklist sequence shifts to a different source-component (see "Source-Components" above) — i.e. the next dependency-ordered item cannot be worked in the current component because it lives in another component — the item whose completion triggers the shift must end with an explicit handoff callout naming the destination component, milestone, and work item ID. Use the reciprocal form on the destination side: the destination's first dependent item must carry a `CROSS-COMPONENT PREREQUISITE` callout naming the source component / item that must land first, and (if control returns) a `CROSS-COMPONENT HANDOFF` callout at the end pointing back. Recommended format (markdown blockquote so it stands out when scanning):
  > **➡ CROSS-COMPONENT HANDOFF:** next work is in component `<component-path>` → `<milestone-id>` → `<work-item-id>` (`<short title>`). See [`<path-to-CHECKLIST.md>`](...).
  The goal is that a reader executing a checklist linearly never has to infer cross-component dependencies from surrounding prose — the boundary is always called out at the exact item where the handoff occurs.

## 7-bit ASCII only in Copilot-maintained planning and design docs

Every Copilot-maintained planning and design document **must contain only 7-bit
clean ASCII** (every byte in `0x00`-`0x7F`; no code point above `U+007F`). This
governs:

- checklist files: `CHECKLIST.md`, `CHECKLIST-<feature>.md`,
  `COMPLETED-CHECKLIST.md`;
- plan indexes: `PLANS.md`, `COMPLETED-PLANS.md`;
- design docs: `DESIGN-NOTES.md`, `DESIGN-NOTES-*.md`, `DESIGN-RATIONALE.md`,
  `DESIGN-INSTRUCTIONS.md`, any other `DESIGN-*.md`, and every file under a
  `design-sessions/` directory;
- the test-tracking siblings `UNRESOLVED-TEST-FAILURES.md` /
  `RESOLVED-TEST-FAILURES.md`.

Why: non-ASCII punctuation in these files has repeatedly been round-tripped
through the wrong code page by some tool and corrupted into multi-layer mojibake
that is expensive to detect and repair. 7-bit ASCII round-trips cleanly through
every editor, code page, and shell, so this class of damage cannot recur.

Write ASCII spellings instead of the non-ASCII characters that tend to appear in
prose (described by name + code point so this rule file stays ASCII too):

- em dash (U+2014) / en dash (U+2013)  ->  `--`, or a single `-`
- right arrow (U+2192)  ->  `->`  ;  left arrow (U+2190)  ->  `<-`
- set membership (U+2208)  ->  the word `in`
- multiplication sign (U+00D7)  ->  `x` or `*`
- curly quotes (U+201C/U+201D/U+2018/U+2019)  ->  straight `"` / `'`
- horizontal ellipsis (U+2026)  ->  `...`
- non-breaking space (U+00A0)  ->  a normal space (U+0020)
- any other code point above U+007F  ->  its ASCII equivalent or a short word.

Apply this to the content you author or edit in these files going forward.

## Markdown cross-references must be clickable links

Every time a project document (CHECKLIST.md, COMPLETED-CHECKLIST.md, PLANS.md,
COMPLETED-PLANS.md, DESIGN-NOTES.md, DESIGN-RATIONALE.md, COMPONENT.md, README.md,
design-session files, or any other repository markdown file) **mentions another
markdown document by name, that mention MUST be written as a clickable markdown
link with a relative path** — never as bare text and never as an inline-code
filename. This lets a reader Ctrl+Click (Cmd+Click) the reference in VS Code to
open the target document.

Rules:
- **Relative paths only**, resolved from the directory of the *referencing*
  file. A sibling document in the same directory is `[NAME.md](NAME.md)`; a
  document in another crate is `[NAME.md](../<other-crate>/NAME.md)`. Do **not**
  use absolute paths, `file://` URIs, or workspace-root-relative paths unless a
  relative path is impossible.
- **The link text is normally just the document's file name** (e.g.
  `[DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md)`), so the reader still sees
  which file is meant. Any qualifier that disambiguates *which* copy (e.g.
  "acadia's", "this crate's") stays as prose **outside** the link.
- **Verify the target exists** before writing the link, so links are never
  broken on creation. Links also break *later*, when the target is deleted or
  renamed -- see "Retiring a file: repoint live references, leave historical
  ones alone" under CHECKLIST file hygiene for which references to repair and
  which to leave as history.
- **When a reference points at a specific decision ID or heading**, keep the ID
  as inline code *after* the link (e.g.
  `[DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md) → `D300``). Linking to the
  heading anchor (`...DESIGN-NOTES.md#d300----anchored-artifact-reads`) is encouraged when the anchor
  slug is known to be correct, but linking to the file is always acceptable.
- **This applies to references to non-markdown repository files too** (source
  files, `Cargo.toml`, JSON grafts) whenever the intent is "go look at this
  file" — make them relative-path links as well. Inline code without a link is
  reserved for symbols, identifiers, and snippets, not for file references the
  reader is expected to open.
- When **editing or adding to an existing document**, convert any bare or
  inline-code markdown-document references you touch into links as part of the
  edit; do not leave new non-linked cross-references behind.

## Coding conventions

### Source module size — measured in bytes, split at fracture points

Line counts are a poor proxy for module complexity, and an arbitrary line cap is a poor
design criterion. Judge a source file instead by its **size in bytes on disk**, on a
power-of-two scale. The scale applies to a **single source file**, not to a directory or a
crate. Crossing a threshold does not mandate a split; it raises the *pressure* to find a
fracture point, and that pressure compounds at every subsequent threshold.

**The scale is a smoke alarm, not a blueprint.** It decides *whether to go looking*; it never
decides *where to cut* or *how many parts to make* — that comes entirely from the fracture
points the file already has. A file may sit two thresholds up and still be right to leave
whole, and a file below every threshold may still be worth splitting when it obviously wants
to be two things. Never reach for the byte count to justify a cut.

| Size on disk | Name | Default posture |
|---|---|---|
| < 32 KiB | normal | No action. |
| >= 32 KiB | **large** | Look for a fracture point the next time you materially edit the file. Split if a clean one exists; leaving it whole is fine if none does. |
| >= 64 KiB | **extra large** (XL) | The default answer flips: **split**, unless you can name what makes the module indivisible. |
| >= 128 KiB | **XXL** | Split, or record a decision in the nearest DESIGN-NOTES.md naming the specific property that makes this module irreducible. |
| >= 256 KiB | **XXXL** | As XXL, and the file is a standing defect: queue the split as a CHECKLIST.md item rather than deferring it again. |

The scale continues by powers of two — each doubling adds one "extra" (32 KiB * 2^n = n
"extra"s) — and each step raises the bar for the justification that lets the file stay whole.

Rules:

- **Measure, do not estimate.** Use `tpu_stat_file` (`size`) or `tpu_count_file`; never infer
  size from a line count or from how large the file "feels".
- **Check at the moment you add.** Whenever you materially add to a source file, check whether
  the addition pushes it across a threshold. That is the moment to decide — not a later audit
  that nobody is scheduled to perform.
- **Split at fracture points, never at byte budgets.** A fracture point is a subset of the
  module with its own coherent responsibility and a narrow interface to the rest. If there is
  no fracture point, do not cut. **Never** split a module by relocating its trailing N bytes
  into a new file: a mechanical cut yields two modules that are each individually
  incomprehensible, which is strictly worse than one large coherent module.
- **A split is a refactor and lands on its own commit.** The module's public surface does not
  change, behavior does not change, and no other work rides along. Every split must also carry
  the provenance trail described below. This is the one deliberate exception to committing
  only at milestone boundaries: before splitting, `git stash push --include-untracked` the
  in-flight milestone work, make the split commit on a clean tree, then `git stash pop`.
- **A split must not subvert layering.** See PLATFORM INTEGRITY above: if the only available
  fracture point would collapse or re-route a layer boundary, raise it with the engineer
  instead of taking it.
- **Some modules are legitimately irreducible** — a single exhaustive dispatch, a data table,
  a state machine whose states only make sense together. Say exactly that in the DESIGN-NOTES
  decision when such a module crosses XXL. **Generated code is exempt at every threshold.**
- **Test modules split more readily, not less.** They sit on the same scale, and their fracture
  points (by feature area, by scenario) are usually obvious — a large `tests.rs` should almost
  always become a `tests/` directory of focused siblings. For an **integration** test there is
  a layout trap: a test crate root resolves `mod x;` against the directory that *contains* it
  (`tests/`), not against a directory named after itself, so `tests/<name>/<part>.rs` is
  unreachable from `tests/<name>.rs` and parts placed in `tests/` would each be compiled as
  their own test target. `git mv tests/<name>.rs tests/<name>/main.rs` first — cargo
  auto-discovers that layout and the target keeps its name — then the parts resolve normally.
- **Prefer facade-preserving splits.** Promote `src/foo.rs` into `src/foo.rs` plus
  `src/foo/<part>.rs` (Rust 2018 style — no `mod.rs`), keeping `foo` as the facade that
  re-exports the parts, so every existing `use` path keeps working. A glob re-export
  (`pub use self::<part>::*;`) preserves both the paths and the public surface without naming
  each item, and it also puts the parts' public types back in scope for the facade's own code.
- **Watch the facade: if it is still the largest part, you split the periphery, not the module.**
  Shared items accumulate in the parent, so a split can report success against the byte scale
  while leaving the real mass exactly where it was. When the facade remains the biggest file in
  its own family, say so plainly rather than counting the split as done: that residue *is* the
  module. Either find the fracture inside it, or record what makes it irreducible.

#### Before you cut: check the privacy graph

Coherent responsibility is what makes a fracture point *worth* taking; **reachability** is what
makes it *possible*. In Rust that is decided by item visibility, and the shape is asymmetric:

- a child module **can** see its ancestors' private items;
- a parent **cannot** see its children's private items;
- siblings **cannot** see each other's private items.

So before cutting, list every **private** item defined in the candidate subset, and every
private item the subset references, then check the direction of each reference:

- defined in the subset, referenced only inside it — moves cleanly;
- defined in the parent, referenced by the subset — fine, no action;
- defined in the subset, referenced by the parent or by another part — **the cut is not viable
  as drawn**: that item belongs in the parent.

The reference that is easiest to miss, and the most expensive, is a private *type* named in a
parent-owned struct's field, because it drags its whole cluster back with it. In `canopy`'s
store, `Store` holds a private `Genesis`, whose impl reads `GenesisConfig`'s private fields,
which read `ShireId`'s — so all three types had to stay with `Store` even though the `init` /
`open` paths moved out cleanly. The same applies to private *methods*: a `fn` on the shared
type that two parts both call must live in the parent, not in whichever part happened to
define it first.

When a private item turns out to be shared, **move it to the parent — never widen it to
`pub(super)` to make the cut work.** Widening is a visibility change, which a pure relocation
forbids (see below); and "this item moved to the parent" is the honest signal that it is
shared, where a widened item silently stays in a module that no longer owns it.

Running this check with grep before cutting costs a minute. Discovering it from `E0624` /
`E0425` afterwards costs a full reset-and-retry of the split, and it has already cost three of
them on one file.

### Splitting a module — the mandatory provenance trail

Git has **no model of file identity**. It stores whole-tree snapshots and *infers* renames
and copies at query time by content similarity; nothing about a split is recorded in the
commit. A split is a **copy**, not a rename (the source file survives), so `git log --follow`
and GitLens's file history silently dead-end at the split commit and every line in the
extracted file appears to have been authored by whoever performed the split. To stop that,
three layers of trail are **mandatory** on every split, and all three land in the split
commit itself:

1. **Pure relocation** — so git's copy-detection heuristics can recover the lines.
2. **Commit trailers** — the only durable, machine-readable, non-heuristic record.
3. **A provenance header** in each extracted file — the human-visible pointer.

**1. Pure relocation.** The split commit moves bytes and does nothing else: no reflow, no
rename of any item, no visibility change, no behavior change, no unrelated work. Preserve
each moved block's **indentation level** so `cargo fmt` is a no-op on it (if `fmt` reflows
moved code, the cut was not clean — reconsider the fracture point). Keep `use`/import fixups
to the literal minimum needed to compile: **every line you touch is a line that loses its
provenance**.

**2. Commit trailers.** The split commit's message ends with a trailer block: one
`Split-Source:` line naming the file the content came from, and one `Split-Into:` line per
extracted file, all paths repository-relative. The block must be the **final paragraph**,
preceded by a blank line, containing **only** `Key: value` lines — a stray prose line inside
it makes git refuse to parse any of it.

**3. Provenance header.** Each extracted file carries, immediately after the copyright line,
a single comment naming its immediate source and the **pre-split commit** (that is `HEAD`
before you commit, so it is knowable while you author):

```rust
// Copyright (c) Michael Grier.
// Split from store.rs at ffffeeee.
```

The facade file needs no header — its `mod` declarations are the forward pointer. When a file
that was itself split is split again, the header names only its immediate source; the chain
is walked one hop at a time.

#### Procedure — command-line git (PowerShell)

1. Start from a clean tree (stash in-flight milestone work first, see above); the split is
   its own commit.
2. Capture the pre-split commit for the header:
   `git --no-pager rev-parse --short HEAD`
3. **Move the bytes by copy-then-delete, never by retyping.** `tpu_copy_file` the whole source
   file onto each new part, then issue **one** `tpu_edit_file` per part that deletes everything
   that part does not keep. All positions in a `tpu_edit_file` call reference the *original*
   file and are applied without interfering, so a single call carries every `delete` range plus
   the `insert` of the part's header. Then do the same to the source file itself, deleting the
   complement of what the facade keeps. This is what makes byte-identical relocation practical:
   re-emitting the text through the model reflows it, and blame then attributes the whole file
   to the split.
   - Include one adjacent blank line in each deleted range, or `rustfmt` collapses the doubled
     blank afterwards and adds noise to the diff.
   - When methods move out of an `impl` block, give each part its own `impl Type {` / `}`
     wrapper — two new lines — so the moved methods keep their original indentation and
     `cargo fmt` stays a no-op on them.
   - Derive the ranges from doc-comment starts, not from the `fn` / `struct` line, so each
     item's doc block travels with it.
4. Add the provenance header to each extracted file; wire the facade's `mod` / re-exports.
5. Run the Rust pre-commit gate (`cargo_fmt`, then `cargo_clippy --all-targets`) and the
   in-scope tests. `cargo fmt` should produce no diff inside the moved blocks.
6. Write the message to a scratch file (multi-line messages must never go through `-m`):

   ```
   Split store.rs: extract recovery and scrub

   Pure relocation, no behavior change. store.rs had reached 71 KiB (XL);
   recovery and scrub each carry their own responsibility behind a narrow
   interface.

   Split-Source: crates/canopy/src/store.rs
   Split-Into: crates/canopy/src/store/recover.rs
   Split-Into: crates/canopy/src/store/scrub.rs
   ```

   then `git add -A` the source, the new files, and the facade, and
   `git commit -F .scratch/split-commit.txt`.
7. **Verify the trailers parsed** — this must echo every extracted path; empty output means
   the block is malformed:
   `git --no-pager log -1 --format="%(trailers:key=Split-Into,valueonly)"`
8. **Verify blame traces through the split** — count the extracted file's lines still
   attributed to the split commit; expect only the provenance header and whatever you had to
   edit to compile:

   ```powershell
   $split = git --no-pager rev-parse --short HEAD
   git --no-pager blame -w -C1 -C1 -- crates/canopy/src/store/recover.rs |
     Select-String "^$split" | Measure-Object | Select-Object -ExpandProperty Count
   ```

   **Both parts of `-C1 -C1` matter, and they mean different things.** *Repeating* `-C`
   widens the search: a single `-C` only inspects files modified in the same commit, while
   **twice** makes blame inspect the other files in *the commit that created this file*,
   which is exactly the split case. The *numeric* argument is the detection threshold --
   the minimum run of alphanumeric characters a moved block must contain before blame will
   attribute it -- and it defaults to **40**, which is far too coarse for real source: code
   is full of short lines (`}`, `);`, `.expect("...")`) and whole extracted modules silently
   fail to trace at the default. Measured on a 16-way split of a large `tests.rs` in the repository these instructions came from:
   at the default threshold only 6 of 16 files traced at all, while `-C1 -C1` recovered
   **5,267 of 5,669 moved lines (93%)**.

   Expect a small residue attributed to the split: the provenance header, anything you had
   to edit to compile, and some short boilerplate lines that fall below matching confidence.
   If instead *most* of the file is attributed to the split, the relocation was not pure --
   fix it and `git commit --amend -F .scratch/split-commit.txt` (the commit has not been
   pushed yet).

#### Procedure — VS Code with GitLens

Same three layers; only the mechanics differ.

- **Authoring the message with trailers:** in the Source Control input box, `Enter` inserts a
  newline and `Ctrl+Enter` commits — so type the subject, a blank line, the body, a blank
  line, then the `Split-Source:` / `Split-Into:` lines. (The scratch-file + `git commit -F`
  route above is equally acceptable and is required if you are driving git from the terminal.)
- **Confirming the trailers:** open the GitLens **Commit Graph** or **Commit Details** view
  and check the trailer lines appear verbatim at the end of the message body.
- **Blame that traces through the split:** GitLens does **not** pass `-C` by default. The
  repository does not check in VS Code settings, so set
  `gitlens.advanced.blame.customArguments` to `["-w", "-C1", "-C1"]` in your user or
  workspace settings; with that in place, *GitLens: Toggle File
  Blame* on an extracted file attributes the moved lines to their original commits and authors
  rather than to the split. If you see the split commit on every line, either the setting is
  not active or the relocation was not pure. (See step 8 above for why the threshold must be
  `-C1` and not a bare `-C`.)
- **File history:** GitLens's file history follows renames only, so it will still stop at the
  split — that is expected and is why the trailer exists. Read `Split-Source` off the split
  commit and open the history of that path to continue.
- Note that **GitHub's web blame does not apply `-C`** and will always show the split commit
  as the origin. Local blame and GitLens (configured as above) are the tools of record.

#### One-time repository setup

```powershell
git config --local diff.renames copies
git config --local alias.blame-split "blame -w -C1 -C1"
```

The alias gives `git blame-split -- <file>` as shorthand for the verification in step 8.

### No manifest numeric constants in source code

Never write bare integer or byte literals as discriminants or protocol tags inline in logic code.
Instead, use **either** a named `#[repr(u8)]` enum **or** a `mod` of typed `const` values, and use
those named identifiers everywhere — in match arms, `vec![]` pushes, assertions, and doc tables.
Both approaches are acceptable; consistency within a single file or module is what matters.

**Bad:**
```rust
v.push(4u8);   // what is 4?
vec![255u8]    // magic
assert_eq!(key, vec![0u8]);
```

**Good (enum approach):**
```rust
#[repr(u8)]
enum ValueKeyTag { DbNull = 0, Text = 4, Err = 255, ... }

v.push(ValueKeyTag::Text as u8);
vec![ValueKeyTag::Err as u8]
assert_eq!(key, vec![ValueKeyTag::DbNull as u8]);
```

**Good (const approach):**
```rust
mod tags {
    pub const DBNULL: u8 = 0;
    pub const TEXT:   u8 = 4;
    pub const ERR:    u8 = 255;
}

v.push(tags::TEXT);
vec![tags::ERR]
assert_eq!(key, vec![tags::DBNULL]);
```

This rule applies to all binary encoding schemes, wire protocols, file format tags, sort-key type
bytes, and any other place where a numeric value carries identity meaning. The enum or const module
is defined in the same file or module as the logic that uses it, and its doc comment must note that
changing any value is a breaking change.

## Design Autonomy — Behavior is owned, never inherited from dependencies

We **define** our behavior. We **choose** dependencies that can satisfy our definition.

It is never acceptable to describe our behavior as "whatever crate X does" or "we delegate to
library Y." That framing surrenders our autonomy to decide what is correct for our users and makes
it impossible to reason about correctness, versioning risk, or future migration.

The correct framing is always:
1. State **what our specified behavior is** (inputs we accept, outputs we produce, errors we raise).
2. Note **which dependency is used to achieve it** and that the dependency was chosen because its
   behavior matches our specification.
3. If a dependency's actual behavior diverges from our specification, the dependency is wrong,
   not our specification. We either constrain the dependency, wrap it, or replace it.

We may align our specification with a dependency's behavior when that behavior is sensible for our
users — but the specification must still be written down explicitly and owned by us. When a
dependency is upgraded or replaced, our specification does not change; only the implementation does.

This applies everywhere: file formats, parse rules, error messages, wire protocols, encoding choices.

## Mono-repo bug policy — fix the layer, don't work around it

All crates in this repository are under active development. When work in one component
reveals a bug or deficiency in an underlying layer (another crate in the repo), **fix it
at the source** rather than working around it in the consuming crate. The whole point of
the mono-repo is that we own every layer and can change them together.

If the fix demands significant refactoring that would derail the current task, raise the
issue back to the engineer driving forward progress so we can decide together whether to
fix it now or defer it. But the default is always: fix the bug where it lives.

## Source-Components

- Source-Components are directory hierarchies in the repository rooted at some directory.
- Source-Components are identified by the presence of either a Cargo.toml file or a COMPONENT.md file in the directory.
- The root of the repository contains a Cargo.toml file, so the entire repository is a source-component, but there are also smaller source-components within the repository which may have their own Cargo.toml or COMPONENT.md files.

Examples:
- `src/tools/csv/` (has COMPONENT.md)
- `src/tools/csv/csv/` (has Cargo.toml)

## Always plan
- Always form a plan in the form of a CHECKLIST.md, at the lowest common source-component for the change
- Keep the plan up to date as you execute on the plan
- Keep a file at the root of each component, called PLANS.md, which tracks all the CHECKLIST.md files in the repository and their status (not started, in progress, completed). If it does not exist, create it. If it does exist, update it with the new CHECKLIST.md file and its status.
- When a CHECKLIST.md file is completed, move it to a table in a different file called COMPLETED-PLANS.md in the same directory, with a brief summary of the work completed, and remove it from PLANS.md.



PLANS.md format (markdown table):
| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|

COMPLETED-PLANS.md format (markdown table):
| Path to CHECKLIST.md | Completion Date | Brief description | Design Notes |
|---|---|---|---|

Status values: "not started", "in progress", "completed"

Design Notes column: Path(s) to DESIGN-NOTES.md file(s) that document the work, or "N/A" if none exist

## Plan sizing

If a plan exceeds roughly 10 work items or 3 levels of grouping/nesting, checkpoint it
into a CHECKLIST.md file in the repository before continuing. The goal is that the plan
survives a lost session — if the plan only exists in the chat, it will be lost.

## Design notes are not a work queue

Design notes (DESIGN-NOTES.md, DESIGN-RATIONALE.md, and related files) record *decisions*
— what was chosen and why. They steer future work, but they do **not** schedule it. The
only mechanism that queues work on existing code is a CHECKLIST.md item. A decision that is
recorded only in a design note, with the work it implies never transcribed into a checklist
item, is effectively orphaned: nothing will ever cause that work to be picked up.

This matters because the repository is worked by multiple people and multiple automated
agent sessions, often in parallel and on different machines. None of them share local or
session-private memory. The only directive any contributor — human or agent — can rely on
seeing is what is committed to the repository. Therefore work must be queued in committed
CHECKLIST.md files, never parked in an agent's memory, a chat thread, or a design note that
no one is obligated to act on.

When recording a decision that implies a change to existing code:

- In the same change that records the decision, ensure the implied work exists as a
  CHECKLIST.md item (creating or updating the checklist as needed), and reference the
  decision from that item so the two can be traced to each other.
- If a decision deliberately schedules **no** work — a reservation, a deferral, or a
  "leave as-is for now" choice — state that explicitly in the decision so the absence of a
  checklist item is visibly intentional rather than an oversight.

A component may layer additional, stricter conventions on top of this rule (for example, a
required cross-reference syntax between decision IDs and checklist items). Follow the
nearest applicable component instructions in addition to this baseline.

## Agent memory is scratch: promote what matters into the repository

An assistant's memory files (`/memories/`, including the repo-scoped and session-scoped
tiers) are **private, unversioned, machine-local, and invisible to the engineer**. They are
the same category the rule above forbids work from living in: "an agent's memory". They are
useful *within* a session -- for a gotcha you hit once and must not repeat -- and they are
worthless as a record, because no other contributor, machine, or future session is
obligated to or able to see them.

**Anything in memory that a different contributor would need is unrecorded until it is
committed.** Treat memory as a staging area, and promote out of it continuously rather than
at the end of a session that may never get an end.

Where each kind of thing goes:

| What you learned | Where it belongs |
|---|---|
| A decision, or *why* something is the way it is | the nearest `DESIGN-NOTES.md` (see "Design note files") |
| Work that someone must do | a committed `CHECKLIST.md` item |
| How to build, test, or drive the tooling | this instructions file |
| Why an approach was rejected | the decision that rejected it, as a named alternative |
| A trap in a dependency or platform | a code comment at the site, **and** the decision that relies on it |

Rules:

- **Promote at the moment of learning, not later.** The session may end abruptly; a note
  that only exists in memory when it does is lost, and the next session pays the same cost
  again. If a fact was worth writing to memory, ask immediately whether it is worth
  committing -- and it usually is.
- **Never cite memory as the record.** "I noted that in memory" is not a substitute for a
  design note or a checklist item, and must not be offered as one.
- **Never let a chat message be the only home for an observation.** Raising something in
  conversation and moving on is the same failure as parking it in memory. If it matters,
  it lands in a file in the same change; if it does not matter, say so and drop it.
- **Prefer deleting a memory entry once promoted**, or reduce it to a pointer at the
  committed location, so the two cannot drift and the memory does not become a competing
  source of truth.
- **Correct the repository when memory turns out to be wrong**, not just the memory. A
  wrong belief that was acted on has usually left a trace in a comment, a decision, or a
  checklist item, and that trace is what the next contributor will read.

## CHECKLIST file hygiene

CHECKLIST files are **action-only**: they contain pending, in-progress, and recently
completed (`[x]`) items awaiting migration to `COMPLETED-CHECKLIST.md`. Completed items
must be moved to `COMPLETED-CHECKLIST.md` when a group is fully done (see below), with one
exception: a **large** completed item is moved *immediately* and replaced in place by a
one-line **stub-with-link** (see "Completed-item stubs (move-with-link)" below). Apart from
those single-sentence stubs, never leave historical records, prose summaries, rationale, or
context in a CHECKLIST file.

Checklists for work more than 2-3 items long should be organized into milestones.
Milestones should generally be sized to about 5 work items (suggestion, not a rule) and
should end with integration tests when possible.

At the end of every milestone, the following steps are **implicit** and must NOT be written
as checklist items:

1. **Build the default workspace (no warnings), both debug and release.**
   Fix **all** warnings that appear, even those unrelated to the milestone's
   changes. This is an ordinary (incremental) build — do **not** discard build
   artifacts first; we no longer force a clean rebuild to re-emit warnings. The
   exact commands depend on the language toolchain — see the language-specific
   instructions for the mapping (for Rust this is `cargo check --all-targets` and
   `cargo check --all-targets --release`; the Rust pre-commit gate is stated inline in
   this file).

   **Scope = the default workspace, NOT every member.** "The default workspace" is the
   set of crates the build tool selects when given no explicit package/scope flag (for
   Cargo: the `default-members` list). This workspace currently declares no
   `default-members`, so the default set is every member. If a member is ever excluded
   from the default set because it is expensive to build (for example, a crate with a
   large codegen build script), that exclusion is intentional and must be respected.

   **Do NOT broaden the scope to all members.** For Cargo specifically: run plain
   `cargo check --all-targets` (which honors `default-members`). **Never add
   `--workspace`** (nor enumerate every package with repeated `-p`) for a
   milestone-boundary build — doing so overrides `default-members`, drags in the
   intentionally-excluded slow crates, and has previously caused builds to appear to
   hang. `--all-targets` (tests/examples/benches) is fine and expected; `--workspace`
   (all members) is not.
2. **Test only the in-scope crate / source-component**, not the whole default workspace.
3. **Build the in-scope crate's documentation, and resolve every broken link it reports.**
   For Rust this is `cargo doc -p <crate> --no-deps`, and the gate is specifically rustdoc's
   **`unresolved link`** class: a doc comment naming an item that does not resolve.

   **Why this is a gate and not tidiness.** A link like ``[`Store::write_document`]`` is a claim
   that the API exists. When the item is deleted, the claim becomes false and the doc starts
   lying -- but **`cargo check` and `cargo clippy` say nothing about it**, so it survives every
   other gate in this list. That is not hypothetical: retiring the partition tree left the canopy
   crate docs advertising two deleted methods and a deleted error variant, and they were found
   only because someone happened to run `cargo doc` by hand while editing the same comment.

   **Scoped to the crate you are working in, deliberately.** Other crates carry unresolved links
   today, so a repository-wide form of this gate would fail on work that has nothing to do with
   them. Scoping it to the in-scope crate -- the same scope rule step 2 already uses for tests --
   means the debt is cleared by whoever is already in that code, and never blocks unrelated work.

   **Not gated (yet):** rustdoc's other classes -- `redundant explicit link target`, and public
   documentation linking to a private item. Both are worth fixing when you see them; neither makes
   a document assert something untrue, which is what earns a gate. **Generated code is exempt**,
   as everywhere else in these instructions: a `.proto` comment rendered into Rust can trip
   rustdoc's HTML lint, and the fix belongs in the `.proto` if anywhere.
4. **Sync with origin**: `git fetch`, then rebase the current branch onto the updated
   upstream tip (`git rebase <upstream>`, no flag) or merge it (`git merge --no-edit`),
   resolving any conflicts, then push.
   Pushing is permitted at milestone boundaries without further confirmation; outside
   milestone boundaries, follow the standard "ask before pushing" rule.

These are standard procedure, not work items. Checklists contain only substantive work.

Work items in a milestone must be self-contained and all work items must be in dependency order.

### Blank line between multi-line items

**Separate multi-line checklist items with a blank line** so the division between one item and the
next is visible at a glance. A *multi-line item* is any `- [ ]` / `- [x]` whose body wraps onto
continuation lines or carries a nested sub-list — the "big work items" (a milestone's real
deliverables, an item with a `**Gap:**` / `**Target:**`-style body, a done-note, a decision
write-up). Two such items run together with no separator make it hard to see where one ends and the
next begins.

- **Required between** adjacent multi-line items (and between a milestone's last item and the next
  `## ` milestone heading).
- **Not required between** consecutive **single-line** items — a compact run of one-liners may stay
  dense (a blank line there is optional, never wrong).
- **Never within** an item — its own continuation lines stay contiguous; the blank line goes only
  *between* items.

```
- [ ] **ITEM-A** — a short one-liner.
- [ ] **ITEM-B** — another one-liner.

- [ ] **ITEM-C** — a big item whose body wraps onto continuation lines
  with **Gap:** / **Target:** prose.

- [ ] **ITEM-D** — the next big item, clearly divided from ITEM-C by the blank line above.
```

Repository-wide standard for all `CHECKLIST` / `COMPLETED-CHECKLIST` files. Apply it to items you
create or edit from now on; there is **no** obligation to reflow existing checklists.

### Checked means done — the move-or-spawn rule for deferred work

A checklist item is checked `[x]` **only when its own action is finished**. "Decided to defer,"
"does not apply yet," "no action needed now," or "kept open by a reserved field" is **not**
completion, and must never be recorded by checking the box while leaving the item sitting in a
milestone it does not belong to. Conflating *deferred* with *done* — and leaving post-deliverable
work parked in an early milestone — is the recurring defect this rule exists to stop. Every time
you are about to check, skip, or defer an item, first decide which case you are in:

- **There is still an action to take in *this* milestone** → leave the item **unchecked**. It is
  pending work and stays open until that work is done.
- **There is no action to take in this milestone** → the item does **not** belong here. Do one of:
  1. **Move** the item to the milestone in which it will actually be acted upon (use the `M{n}+` /
     `M∞` notation below when that milestone is not yet numbered), **or**
  2. If a real action *was* completed here that merely **spawns** later follow-up — e.g. a design
     decision recorded in a design milestone that implies later implementation, or an ABI-freeze
     obligation met by reserving a field whose *behavioral* decision is deferred — check the item
     `[x]` for the part that is genuinely done **and create a new work item for the follow-up in
     the correct (later) milestone in the same commit.** Never let the follow-up survive only as
     prose in a design note or a "Tracking" paragraph; that violates *design notes are not a work
     queue*.

The post-condition is mechanical and must hold after you touch any item: **no checklist contains a
checked item whose remaining work is unscheduled, and no milestone contains a no-action item that
belongs to a different milestone.** A deferred item is therefore *always* either moved, or replaced
by a checked-plus-spawned pair — never silently checked in place.

### Milestone naming for future and horizon work

Work is often gated on a deliverable that is not yet a concrete numbered milestone. Do not park it
as an unchecked item in the current milestone (that falsely implies the current milestone is
incomplete), and do not invent a fake number. Use one of two explicit placeholders, both visually
distinct from real milestones (`M1`, `M2.3`, …):

- **`M{n}+`** — read "after milestone *n*" — for work gated on a *specific* deliverable. Example:
  `M1+` = "to be done in some milestone after M1." The `+` names the gating milestone, sorts
  immediately after `M{n}`, and is an explicit **placeholder that graduates** to a real number once
  that later milestone is authored.
- **`M∞`** — read "the horizon" — a single terminal bucket per checklist for genuinely **ungated**
  "someday/maybe" work with no identified predecessor deliverable.

Items in `M{n}+` / `M∞` are **parked, not pending**: they are not open obligations of any current
milestone, and the milestone that unblocks them **pulls them in (graduates them to a numbered ID)**
when it is authored. Rejected alternatives, recorded so they are not re-proposed: `M.Next`
(relative and ambiguous — "next" relative to which milestone? — and two files' `M.Next` mean
different things); `M{n}.5` (implies a *defined* intermediate milestone and collides with the
`RC-1.1` decimal sub-step notation); a bare `Backlog` / `TODO` (loses the gating deliverable and
does not sort with the milestones). `M{n}+` keeps the gate visible and graduates cleanly; `M∞` is
the catch-all when there is no gate.

### Completed-item stubs (move-with-link)

To keep the active checklist navigable without losing the in-place spine of "what we recently
did," a **large** completed item is relocated to `COMPLETED-CHECKLIST.md` the moment it is done
(rather than waiting for its whole group to finish) and a one-line **stub** is left in its exact
place, in dependency order. "Large" means the item's full text is **longer than 100 characters**
*or* it has sub-items. Short, single-line items are **not** stubbed — they stay checked in place
until their group migrates wholesale, exactly as before.

When a large item completes:

1. **Move** the item into `COMPLETED-CHECKLIST.md`, under the current `## Moved YYYY-MM-DD — …`
   date group, as a `###` heading that **opens with an explicit HTML anchor keyed to the
   work-item ID**, then repeats the stub's one-sentence summary, then an inline completion stamp:

   `### <a id="fl-d96"></a>FL-D9.6 — <same one-sentence summary as the stub>. *(completed YYYY-MM-DD HH:MM:SS UTC±hh:mm)*`

   The **HTML anchor — not the heading's auto-generated slug — is the link target**, so the
   descriptive heading text can be arbitrarily long without changing or breaking the anchor. VS
   Code resolves `#fl-d96` to the `<a id>`; the markdown `{#id}` attribute syntax does **not**
   work in VS Code's default engine, so always use the HTML anchor. The anchor id is the
   lowercased work-item ID with periods removed (`FL-D9.6` → `fl-d96`, `FL-C6.2` → `fl-c62`); get
   the timestamp fresh from `Get-Date -Format "yyyy-MM-dd HH:mm:ss zzz"`. The verbatim item body
   **may** be reproduced under the heading when it carries reasoning recorded nowhere else, but is
   **not required** when the authoritative text already lives in a DESIGN-NOTES decision or a
   design-session — in that case add a one-line pointer to it instead.
2. **Replace** the item in place with a single-line stub of the form:

   `- [x] **<ID>** — <one-sentence summary>. → [completed YYYY-MM-DD](COMPLETED-CHECKLIST.md#<id-slug>)`

   where `<id-slug>` is the lowercased ID with periods removed (matching the heading's `<a id>`
   anchor). The stub's summary and the heading's summary are written to **match**. Preserve the
   stub's original indentation if the item was nested. The summary is written **once** at move
   time and the archived heading is **immutable thereafter** (the archive is append-only history),
   so neither the summary nor the link target can drift.

**Never** use a line-number fragment (`#L123`) as the link target — list items shift, headings
do not; anchor only to the heading's `<a id>`. When a milestone finally completes, its remaining
stubs migrate (are deleted) with the group as usual: a stub carries no information not already in
the archive, so nothing is lost.

### Sub-step notation

When a checklist step is broken into sub-steps, always use decimal notation: `RC-1.1`, `RC-1.2`,
`RC-1.3`, etc. (or whatever prefix is in use). Never use lettered sub-items (`RC-1a`, `RC-1b`) or
nested bullet lists to represent sub-steps. This applies both to CHECKLIST files and to any inline
step breakdowns described during planning.

When a group of related items is fully complete:
1. Move the completed group to `COMPLETED-CHECKLIST.md` in the same directory.
2. Prefix the moved block with a heading: `## Moved YYYY-MM-DD — <brief description of what was done>`.
3. `COMPLETED-CHECKLIST.md` is **append-only**; always add new groups at the bottom.
4. Leave only the remaining pending or in-progress items in the source `CHECKLIST.md`.

Named feature files (`CHECKLIST-<feature>.md`) should be **deleted entirely** once all items are
complete. Move their content to `COMPLETED-CHECKLIST.md` in the same directory before deleting.

### Retiring a file: repoint live references, leave historical ones alone

Deleting a file breaks every link that pointed at it. **In the same commit that retires a
checklist, repoint the references to it** -- otherwise they rot silently and nobody notices,
because nothing in the build checks a markdown link. Six retired checklists in this repository
accumulated broken links exactly this way before anyone looked.

References split into two populations, and the split is what makes this cheap:

- **Live references are repaired.** Anything outside the archive: `DESIGN-NOTES.md` at every
  level, `PLANS.md`, other `CHECKLIST` files, `README.md`, `FOUNDATIONS.md`, **and rustdoc
  links in `.rs` sources** -- doc-comment links rot the same way and additionally render broken
  in generated docs, so grep the source, not just the markdown. Where the target goes, and
  whether the link *text* changes with it, depends on why the file went away -- see the two
  cases below.
- **Historical references are left exactly as they are.** `COMPLETED-CHECKLIST.md`,
  `COMPLETED-PLANS.md`, and `design-sessions/` record the paths as they stood when each entry
  was written. A link there that no longer resolves is the record being faithful, not the record
  rotting. Do **not** edit them -- that is what append-only means here, and it is why retiring a
  file needs no tombstone or redirect stub to protect the archive.

**Retired because complete, or folded in while still open?** The two read differently, and
using the wrong one produces a link that is either false or misleading:

- **Retired because complete.** The items are in `COMPLETED-CHECKLIST.md` and the prose
  referring to them is *historical* ("delivered in X", "scheduled by X"). Retarget to the
  archive and **keep the link text naming the retired file**, so the sentence still says what
  was true when it was written.
- **Folded into another checklist while still open.** The items moved to a live file and the
  prose is *present tense* ("tracked in X", "that work is X", "the counting is X's"). Here
  link text naming a deleted file is not history, it is a false statement pointing a reader at
  a file that does not exist -- so **text and target both become the new file**. State the
  fold-in in the destination file's header, and mark the source `(retired)` there as inline
  code rather than a link.

**The same split governs renames**, not just deletions: update every live reference to the new
path and leave the archive pointing at the old one.

Record the retired file in `COMPLETED-PLANS.md` as plain text marked `(retired)` -- not as a
link, since the target no longer exists.

**No automated link checker exists in this repository yet**, so verify every link you add
or repoint by hand. A wrong relative depth (`../../` from `crates/<c>/src/` reaches
`crates/`, not the repository root) is as broken as a missing file and is the easiest way to
introduce one, by copying a link between files at different depths. Where an example link is
illustrative rather than navigable, write it as inline code.

## Design note files

Any directory in the repository may have a DESIGN-NOTES.md file.

The DESIGN-NOTES.md file should record design decisions about the code in that directory and its children.

If a decision should be recorded, it should be recorded in a DESIGN-NOTES.md file. The DESIGN-NOTES.md
file to use is either the DESIGN-NOTES.md file in the source-component directory which should be created
if it does not already exist, or if there is an already existing DESIGN-NOTES.md file in any ancestor
directory between the file being changed and the source-component root, use that one instead.

### What to include

The design note files should include anything that a future developer should or may want to know about the
code to help them "get up to speed" or diagnose interesting or bad behaviors.

### What not to include

Like with code comments, don't include super obvious things.

Example: A query processor design note must describe its intent and unique approach in a paragraph, not provide a comprehensive tutorial on the underlying technology or theory. It may include links to external resources for further reading, but should not attempt to teach the reader about query processing in general.

### Three-tier design documentation

Source-components with substantial design history should separate current decisions from
historical rationale using three tiers:

- **Tier 1: `DESIGN-NOTES.md`** — Current canonical decisions. Contains decision indexes,
  compact detail sections stating what was decided and why. Every paragraph must answer
  "what is the decision?" or "what constraint forced this choice?" — not "what else did
  we consider?" Content that answers the latter belongs in Tier 2.

- **Tier 2: `DESIGN-RATIONALE.md`** — Historical record of how decisions were reached.
  Alternatives considered, prior art, design session summaries, evolutionary reasoning.
  Cross-referenced by decision ID from Tier 1. This file is consulted for "why" questions,
  not for forward implementation work.

- **Tier 3: `design-sessions/DESIGN-SESSION-<date>-<topic>.md`** — Raw design session
  transcripts, dated by session. Reference material, not routinely loaded. Stored in a
  `design-sessions/` subdirectory under the source-component root.

When recording a new decision, write to both Tier 1 and Tier 2 in the same commit.
**Never treat Tier 2 or Tier 3 as authoritative for current decisions.** If there is a
conflict, Tier 1 wins.

A source-component may have a `DESIGN-INSTRUCTIONS.md` file specifying additional design
rules — including how these tiers are used — for that component and everything below it.
When working in a directory, locate and follow the nearest `DESIGN-INSTRUCTIONS.md` in
that directory or any ancestor up to the source-component root. These directives are
binding for all work under that directory.

Not all source-components need all three tiers. Small components may have only DESIGN-NOTES.md.

### Design session files

When a design conversation produces extended discussion, exploration, or working-through of a
topic — beyond what fits in a Tier 2 rationale section — capture it as a design session file.

**When to create a session file:**
- The conversation explores a topic in depth over multiple exchanges
- The discussion covers alternatives, trade-offs, or implications that would be valuable
  context for a future reader trying to understand the design landscape
- The topic warrants a standalone record beyond the decision summary in DESIGN-RATIONALE.md

**Naming:** `DESIGN-SESSION-<YYYY-MM-DD>-<topic-slug>.md` (e.g.,
`DESIGN-SESSION-2026-04-06-task-floating.md`)

**Location:** `design-sessions/` subdirectory under the source-component root. Create the
directory if it does not exist. This prevents session files from accumulating in the
component's top-level directory.

**An open question in a session file must be reflected in a CHECKLIST.md.** A design session may
record open questions -- that is often the most valuable part of it -- but a session file is Tier 3
reference material that nobody is obligated to read, so a question living only there is orphaned by
construction. Every open question must therefore have a counterpart in a committed checklist: an
`ODI-n` item when it blocks definition, an ordinary work item when the work is describable and only
the answer is missing. The session keeps the full reasoning; the checklist carries the obligation.
Cross-reference the two so each can be found from the other, and when a question is answered, say so
in both places.

This is the "design notes are not a work queue" rule applied to sessions, and it is the case that
fails most quietly: a session's *decisions* usually graduate into DESIGN-NOTES.md, while its
*questions* sit in a numbered list that reads like a plan and schedules nothing.

**Content:** The session file should be a faithful record of the design discussion — the
reasoning, alternatives, and conclusions as they unfolded. It does not need to be polished
prose, but should be readable by a future developer. Include a brief summary at the top
noting which decisions (D-numbers) resulted from the session.

### Timelines in design documents

A **timeline** is a strict step-by-step account of one scenario: what happened, at which entity, in
what order. Write one whenever a scenario spans more than one process or more than one store,
because prose blurs exactly the thing that matters -- *which* component acted, and *whose* state
changed.

**Every timeline is laid bare, and is NEVER expressed as a delta of another one.** Writing
"identical to T1 through step 11, then ..." is forbidden however much duplication it saves. The
entire purpose of a timeline is that a reader sees the whole scenario in front of them without
reconstructing it from somewhere else; a delta makes the reader do the assembly, and a reader who
assembles it wrong gets an account that is plausible and false. **Duplication between timelines is
expected and is not debt.**

**One row per event, in order**, in a table with these columns:

| column | content |
|---|---|
| step | the ordinal **and** the step's stable id, both in this one column for density |
| actor | the entity the event happens *at* |
| event | what happened |
| durable effect | which store or file changed, or `--` when nothing did |

**`EXTERNAL` is a reserved actor**, for forces outside the system: a person killing a process, an OS
service manager starting one, a machine rebooting, a timer expiring. Naming these explicitly is most
of a timeline's value, because they are the events prose otherwise assumes away.

**The durable-effect column is not optional.** Naming which store changed is what keeps a
multi-store scenario honest, and it is the column that catches one component's state being mistaken
for another's.

**Every step carries a short stable id beside its number:**

- **2-5 characters**, a mnemonic component plus an optional trailing numeric component, **no
  separator** -- `SUB`, `ORD2`, `KILL`, `RCON`, `END2`.
- **The absence of a number is meaningful.** An unnumbered id is the only occurrence of that event
  in its timeline; `ORD1`/`ORD2` mark a repeat.
- **The same event takes the same id in every timeline it appears in.** That is what makes the ids
  worth having at all.
- Step **numbers are positional** and shift when a step is inserted; step **ids are stable** and are
  what every reference cites.

**Cross-timeline references use `T<n>.<ID>`** -- for example `T3.DROP` -- so a reference names both
the scenario and the step, and survives renumbering on either side. Cross-references are encouraged
in the surrounding commentary; they may **never** stand in for a step inside a timeline.

**State each timeline's evidentiary status**, using the same markers as the surrounding document:
`[verified]` (read out of the code), `[recorded]` (rests on a committed decision), `[projected]`
(derived from verified mechanics but never observed). **A projected timeline must never be allowed
to read like an observed one.**

### Design notes are a record of thought in time

**A design note records what was decided, and what was understood, at the moment it was
written.** That is its whole value. `DESIGN-NOTES.md`, `DESIGN-RATIONALE.md`, and every file
under `design-sessions/` are therefore **immutable once committed**, and the lifecycle below
is how they stay current *without* being rewritten.

**The only edits permitted to a committed design point** are corrections of a typo, a
grammatical error, or a plainly wrong fact (a misspelled symbol, a wrong file path, an
inverted "before/after"). Never update a design point because the design changed, because
better wording occurs to you, or because it reads as out of date. Not being current is not a
defect in a record of what was thought at the time — it is the record working.

#### 1. Superseding a design point — the banner goes at the TOP

When a decision is overtaken, **do not edit its body and do not append the correction to the
end.** Add a superseding banner as the **first thing inside the design topic**, immediately
after its heading and before its original text:

```
### D142 -- Both boundaries are rings

> **SUPERSEDED in part, 2026-09-21, by `D287`.** The dispatch-failure taxonomy below is
> two-way; it is three-way. Read `D287` before acting on anything in this decision.

<the original text, untouched>
```

**Position is the whole point, and it is not a style preference.** A reader -- or an agent
loading this file -- acts on a decision the moment it is read. A correction discovered at
the end arrives after the decision has already been applied, and a long decision may mean
it is never reached at all. Putting it first means the warning cannot be missed and its
application can be anticipated. This is a deliberate exception to the chronological
append-at-the-end form used by the observation logs (see "A recorded observation is never
revised" below): a log is read start-to-finish so its newest entry belongs last, while a
decision is jumped to and acted on, so its newest qualification belongs first.

State in the banner **what** is superseded (all of it, or a named part), **when**, and **by
which** decision id. A partial supersede must say which part, or the reader must diff two
decisions to find out.

#### 2. Retiring a design point — when the corrections outgrow it

**The trigger is volumetric: when the accumulated superseding banners and qualifications
exceed the design point they qualify, the design point is retired.** A decision that is
mostly caveats no longer communicates a decision, and each further banner makes it worse.

To retire it:

1. **Move the entire original text**, with all of its accumulated banners, to
   `DESIGN-NOTES-AGED-OUT.md` (Tier 2 and Tier 3 content moves to the aged-out file
   alongside its component's notes). Record the date of the move in `YYYY/MM/DD` format on
   the moved section. The moved text is not edited on the way out.
2. **Leave a broken-heart note in its place** -- a one-line tombstone, so the id never
   becomes a dangling reference and a reader arriving at the old location is routed rather
   than stranded. Use the ASCII marker `</3` (the design docs are 7-bit ASCII; the words
   matter more than the glyph):

   ```
   ### D142 -- Both boundaries are rings

   `</3` **Retired 2026/09/21.** Moved to
   [DESIGN-NOTES-AGED-OUT.md](DESIGN-NOTES-AGED-OUT.md). Replaced by
   [DESIGN-NOTES.md](DESIGN-NOTES.md) `D287`.
   ```

   The tombstone carries exactly two pointers: **where the old text went**, and **what
   replaces it**. If nothing replaces it, say that instead -- "Not replaced; the constraint
   it addressed no longer exists."
3. **Write the replacement as a new decision with a new id.** Never reuse the retired id.

#### 3. The replacement explains the design -- it never narrates the history

**Write the new decision as though it were the first one ever written on the topic.** State
what the design is and what constraint forced it. Do **not** write "instead of `D142`'s
two-way split, we now use three", "previously we thought...", "this replaces...", or any
other comparison to what came before.

Every design point stands as itself, in its own moment. A reader needs to understand the
system, not to be walked through the project's autobiography; a decision written as a diff
against a decision they have not read is strictly harder to act on. The history is not lost
by this -- it is preserved exactly where it belongs, in the immutable record and the
aged-out file, which is *why* that content is immutable in the first place. Carrying the
narrative forward into each successor duplicates the archive into the live document and
guarantees the live document decays.

The **tombstone** may name the predecessor, and the aged-out file's dated heading may name
the successor. That is the full extent of forward and backward linkage; it belongs in those
two places and nowhere else.

## Derived information does not belong in checked-in documents

**Do not write derived information into a repository document.** Derived information is
anything a tool recomputes on demand and that is true only of one moment: test counts and
pass/fail tallies ("3293 of 3294 passed", "431 run, 1 skipped"), timings, file sizes, line
counts, coverage percentages, benchmark throughput, warning counts, "clippy clean",
dependency versions, the number of call sites of a symbol.

It looks like diligence and is the opposite. Its costs are real and its value is close to
zero:

- **It has no lasting value.** A reader a year later cannot act on "3,294 tests passed". The
  suite is a different size, on different hardware, and the number answers no question they
  have. The build is the authority on whether the build passes, and it is always current.
- **It causes churn.** Every unrelated change invalidates it, so either every change must
  also edit the prose (pure noise in every diff), or -- far more often -- nobody does, and
  the document now states something false with all the authority of a committed file.
- **It invites misinterpretation.** A stale count reads as a claim. The next contributor
  measures, gets a different number, and burns time reconciling a difference that never
  meant anything. Worse, a number that *looks* like a baseline gets treated as one.

**Record findings, not measurements.** The test of whether something belongs is not "is it
true?" but **"will this still be worth reading when the numbers have all changed?"** A
finding is a durable statement about how the system behaves:

| Do not write | Write instead |
|---|---|
| "3,293 of 3,294 tests pass" | nothing -- the suite reports this |
| "`canopy` 431 passed, 1 skipped" | nothing |
| "clippy clean; `cargo fmt` no-op" | nothing -- these are gates, not findings |
| "failed 4 of 4 in isolation, 12% of runs before" | "reproduces deterministically in isolation; previously intermittent" |
| "the split traces 5,267 of 5,669 lines (93%)" | keep -- a one-time measurement that justified a documented technique (see the split-provenance section) |

Two narrow exceptions, both of which are findings that merely happen to be numeric:

1. **A measurement that *is* the finding**, where the number is the point and is not
   expected to be re-measured: a benchmark that settled a design decision, a reproduction
   rate that established a defect was real, a before/after that justified a technique. Write
   it in the decision or the test-failure record that rests on it.
2. **A dated observation of a defect or a behavior**, in a file whose purpose is to record
   observations: `UNRESOLVED-TEST-FAILURES.md`, `RESOLVED-TEST-FAILURES.md`,
   `design-sessions/`. A number there is scoped by its date and describes a specific thing
   that was seen, so it cannot go stale.

   This exception is about *what the number describes*, not which file it is in. It does
   **not** cover a routine gate tally that happens to be written in an append-only file.
   **A completed-item record says what was built and why it is believed correct -- never
   that the suite passed.** "Verification: 3,293 of 3,294 passed; clippy clean" in a
   `COMPLETED-CHECKLIST.md` entry is the canonical violation of this rule, not an instance
   of this exception. If a test result is worth archiving, what makes it worth archiving is
   a *finding* ("this item's revert-probe failed exactly the four bracket-dependent tests,
   so the mechanism is pinned rather than the outcome"), and that sentence needs no counts.

### Write the dependency, not the census -- prefer claims that cannot silently go false

The same hazard appears in prose that contains no numbers at all. **"`scrub_content` has no
caller outside its own tests"** is a census of the repository taken on one day. It is true
when written, it becomes false the moment somebody wires the scrub up, and **nothing about
wiring the scrub up prompts anyone to revisit the document that said so.** The reader a year
later cannot tell whether they are looking at a live warning or a stale one, and the document
is now asserting something false with the full authority of a committed file.

Write the **coupling** instead of the **count**: *"this depends on the scrub being driven, so
its convergence inherits whatever drives the scrub."* That carries the identical warning --
the reader still learns the work is parked behind something that may not be happening -- but
it stays true whether the scrub has one caller, none, or a hundred, because a dependency is a
property of the design rather than a snapshot of the tree.

The test to apply before writing a claim into a planning or design document:

> **If someone fixed this tomorrow, would anything make me come back and correct this
> sentence?** If not, the sentence must be one that their fix does not falsify.

| Do not write | Write instead |
|---|---|
| "nothing calls `X`" | "this depends on `X` being driven / having a caller" |
| "`Y` is not wired up yet" | "this is gated on `Y` being wired up" |
| "there is no scheduler for `Z`" | "this assumes `Z` runs on a known cadence, which it does not own" |
| "only the tests exercise this path" | "this path's behaviour under load is unestablished" |

The exceptions are the same as for derived information, and for the same reason: a **dated
observation** in a file whose purpose is to record observations (`UNRESOLVED-TEST-FAILURES.md`,
`design-sessions/`, a `## Moved <date>` archive entry) is scoped by its date and so cannot go
stale -- it describes what was seen then, not what is true now. It is the **undated, present-
tense** census in a live document that this rule forbids.

### A recorded observation is never revised -- only superseded

This is the second half of the same rule, and it governs every append-only document above.

**Once an observation is committed, do not edit it, reword it, correct it, or delete it.**
It is a record of what was believed at a point in time, and its value *is* that it is fixed.
Rewriting history to match present understanding destroys the only evidence of how the
understanding changed -- which is usually the most valuable thing in the file.

When you learn something that contradicts a recorded observation, **append a new dated
entry** that supersedes it, and say plainly what it supersedes and why:

**In a chronological log the supersede goes at the end; in a design decision it goes at the
top.** These files are read differently. A log is read start-to-finish, so its newest entry
belongs last. A decision is jumped to and acted on immediately, so a correction found at the
end arrives too late -- see "Superseding a design point -- the banner goes at the TOP" under
"Design note files" above. The rule against *revising* is identical in both cases; only the
placement differs.

```
- **Update, 2026-09-21 -- the contention diagnosis above is superseded for this failure.**
  It fit every observation, and so does a narrow race whose window widens under load.
  Left in place above as the record of what was believed, not as a current explanation.
```

Superseding is not the same as revising: the old text stays, intact and wrong, with a
pointer forward. A reader then sees both the belief and its correction, which is what makes
the file worth keeping.

Corollaries:

- **Never `git commit --amend` a documentation fix into a commit whose record is already
  written**, and never edit a `COMPLETED-CHECKLIST.md` or `COMPLETED-PLANS.md` entry after
  the fact. If an archived entry is wrong, append a correction under a new dated heading.
- **The only legitimate edits to an append-only file are appends**, plus the specific
  migrations these instructions already define (moving a resolved failure from
  `UNRESOLVED-TEST-FAILURES.md` to `RESOLVED-TEST-FAILURES.md`, moving a completed group
  into `COMPLETED-CHECKLIST.md`).
- **Fix a typo only before the commit that introduces it.** After that it is history.
- This does **not** apply to *live* documents -- `CHECKLIST.md`, `PLANS.md` and `README.md`
  are meant to state the current position and are edited freely as it changes.
- **`DESIGN-NOTES.md` is not such a document, despite being Tier 1.** It is *current* in the
  sense that its decisions are the ones in force, but each decision is still a record of
  thought in time and is immutable once committed. It is kept current by superseding and
  retiring decisions, never by editing them -- see "Design notes are a record of thought in
  time" above. Tier 1 wins over Tiers 2 and 3 on what is *in force*; that is not a licence
  to rewrite it.

## Standing orders on git merge / rebase — ON-GIT-MERGE-OR-REBASE.md

A directory may carry an `ON-GIT-MERGE-OR-REBASE.md` file of **standing orders**: actions
that must be carried out whenever other contributors' commits are integrated into your
working tree for the component it governs. It is located by the **same nearest-ancestor
rule as `DESIGN-NOTES.md`** — the copy in the source-component root, or the nearest one in
any ancestor directory between the file being changed and that root. The filename is
deliberately long and unabbreviated so that a contributor following *either* the git
discipline *or* the design-notes discipline will not miss it.

**When the trigger fires.** The trigger is the *arrival of incoming commits in your working
tree* — via `git merge`, `git rebase`, `git pull`, or a `git fetch` followed by either —
**including conflict-free integrations**. It is *not* the act of running a git command, and
*not* the presence of conflicts: a clean, no-conflict integration can still oblige
follow-up. On every such integration, **before you push or resume your task**, consult the
nearest `ON-GIT-MERGE-OR-REBASE.md` for each component the incoming commits touched
(including the one you are working in) and carry out its orders — **even when those incoming
changes lie entirely outside the scope of what your session set out to do**.

**Why this exists.** Integration pulls in work your task would never otherwise touch, yet
that work can obligate reconciliation of the component's *derived or generated* artifacts.
Example: an incoming commit reshapes test metadata and adds a new test corpus file — that
file's **golden must be regenerated** even though your session changed nothing about that
test. Standing orders record exactly these component-specific post-integration duties so
that every session, human or agent, honors them regardless of its own goal. When you
discover such a recurring duty for a component, add it to that component's
`ON-GIT-MERGE-OR-REBASE.md` (creating the file per the location rule if absent) in the same
change that performs it.

## Quality

When providing testing, always provide extensive testing to test at least 10 normal cases, as well as all identifiable edge cases, unless the
computation required to test the edge cases would be excessive on a modern system. The unit tests for a submodule should be able to complete
in under one second of elapsed time on an AMD Ryzen R7 processor running at 1.5ghz with 16gb of memory.

If there are tests which seem vital that would take longer, put an item in a CHECKLIST.md file with special importance for the user to
decide on whether to include them or not.

In any case, if the test is vital it must be authored and be run as part of the integration tests rather than the unit tests.

### Milestone vs sub-milestone checklist work

When working on checklist items organized into milestones, build and test only the
source-component in scope, not the entire repository.

To complete the milestone, perform the implicit end-of-milestone steps described under
"CHECKLIST file hygiene" above (repo-wide build with zero warnings, in-scope tests,
sync with origin and push).

The in-scope tests at a milestone end include the crate's integration tests; between milestones they
are not part of the per-item loop (see "Integration tests" below).

### Unit tests

Unit tests should always be reproducible and not use random sampling techniques at runtime without the developer's explicit approval and then
it should be recorded in a design note.

**No test names a port, and none binds a port, releases it and binds the number again.** The system
hands out an ephemeral port at `127.0.0.1:0` and the test reads the address it was given
(`local_addr`, or what the code under test reports having bound); a number written into a test is a
collision waiting for the day two tests run at once, and a number released and rebound can be given
to any other process in between. Where a test needs one address to outlive the servers behind it -- a
server that goes down and comes back while a client keeps dialing, or an address handed out before
anything listens -- hold the listening socket for the whole test and forward to whichever server
is behind it, rather than releasing the port and binding the number again. A port in a
configuration value that is only parsed, never bound, is data and is fine.

### Integration tests

Integration tests should use larger scale data.

There is no required minimum, but in general should start with data volume in the hundreds or thousands.

The data does not have to be necessarily stable. A guideline might be that smaller data sets (<10kb) should be checked in whether in
a separate file or somehow encoded in source files. Larger data sets may be generated at run time, whether exhaustively or
via random techniques.

Integration tests that start processes run in parallel with each other, so each process they start
that listens is told to bind port 0 (`127.0.0.1:0`) and is asked what it bound, never given a number
chosen by the test.

**Integration tests are milestone gates, not part of the per-item loop.** They are slow by design
(larger data, real processes, restarts at scale), and running them after every work item adds minutes
to each one for little information. So:

- **Per work item,** run the smallest set that covers the change: format, clippy on the crates
  touched, and the unit tests for the code changed (a filter on the new or affected test module, or
  `--lib` for a crate). Do not run a crate's integration suites, the cross-process tests, or a multi-crate
  run as a habit.
- **At a milestone end,** run the in-scope crate's full suite, integration tests included.
- **Between those, run one when the churn warrants it:** a component has been substantially
  reworked, a boundary it relies on has moved, or a change is the kind unit tests cannot vouch for
  (concurrency, restart and recovery, the wire between processes). Then the run is a deliberate
  point-in-time re-assertion of quality, run to find issues. Say why it was run.
- **Evidence decides the cadence.** If a suite keeps finding real issues, that is reason to keep
  running it more often; if it keeps passing on changes that do not touch what it covers, running it
  is only spending CPU.

Running one is never wrong. The point is that it is a decision with a reason, not a step of the
ordinary process.

## Architectural pre-steps

**Never call `stdout`/`stderr`/`print`/`eprintln` (or the language equivalent) from
more than one site in a tool.** At the first occurrence, introduce an output
abstraction — a writer trait, a sink, or a formatter — and route every subsequent
output through it. The abstraction need not be elaborate (a single trait with one
`write_str` method, or a UTF-8 character stream, is enough); the requirement is that
the storage target (file, channel, stdout, stderr) and the formatting concern be
separable from the call sites that produce content.

This applies to any feature whose output may plausibly need to be retargeted later:
CLI output, log output, diagnostic output, generated artifacts.

## Agency Review -- always persist the report to `.scratch/`

Whenever an Agency Review (`/agency-review`) run completes in this repository,
**save the full rendered review report to a file** under `.scratch/` before
finishing the turn, in addition to showing it in chat. This is agent-side
behavior -- it is not an `agency.toml` field (the `[review.*]` config surface has
no local-file-output option and rejects unknown keys), so it must be enforced as
a standing rule here.

- **Path / name:** `.scratch/CODE-REVIEW-<YYYY-MM-DD>-<HHMMSS>.md`, using the
  local date and time at which the review completed (24-hour clock, zero-padded,
  no separators in the time component -- e.g.
  `.scratch/CODE-REVIEW-2026-08-20-165459.md`).
- **Contents:** the complete review report -- run metadata (change source, scope,
  commit range or PR id, files/hunks, coverage/partial status, surface), every
  finding verbatim (severity, file, anchor, message, and any suggested
  replacement), the "examined and found sound" summary, and the run/transparency
  notes.
- **Applies to every run** regardless of scope (local working tree, staged,
  unstaged, or commit-range) and regardless of finding count -- write the file
  even when there are zero findings.
- `.scratch/` is git-ignored, so these reports are never committed; create the
  directory if it does not exist.

<!-- tpu-mcp:setup:begin -->
<!-- tpu-mcp:setup:version=6.0.0 -->

## File I/O — use `tpu_*` MCP tools, never PowerShell or shell

This workspace runs the **tpu-mcp** MCP server which exposes encoding-aware
file primitives as first-class tools. Plain `Get-Content` / `Set-Content` /
`Out-File` / `>` / `cat` / `sed` round-trip files through the active code
page and silently corrupt UTF-8, UTF-16, smart quotes, em-dashes, and
box-drawing characters. Use the MCP tools instead — they detect, preserve,
and round-trip the file's native encoding and line endings safely.

**Rule:** when working in any project that has the tpu-mcp server registered,
ALWAYS prefer the `tpu_*` tools over PowerShell or shell file commands.

### Version check (do this first)

Every `tpu_*` response begins with an `x-tpu-mcp-invocation` header line
that includes a `tpu_version` field — the version of the running `tpu-mcp`
binary answering the call. The `<!-- tpu-mcp:setup:version=X.Y.Z -->` HTML
comment on the very first line of this block records the version of `tpu`
that wrote this guidance.

**If the two versions differ, stop and report the mismatch to the user
before performing any file mutation.** Include both versions in the
message and suggest the remedy:

- **Binary older than guidance** — rebuild / reinstall the `tpu-mcp`
  extension so its bundled binary matches this guidance, then retry. This
  is the case that has historically wasted whole sessions: a defect the
  user reads about as "fixed" is still present in the running binary
  because the extension's bundled `tpu-mcp.exe` was not updated.
- **Binary newer than guidance** — re-run `tpu setup --inject
  <path-to-copilot-instructions.md>` to refresh this block against the
  newer binary, then retry. Tool descriptions, response shapes, or
  recommended workflows may have changed.

This check is cheap (one field on one line, checked once per session) and
catches version-drift bugs before they manifest as silent behavioural
differences.

| MCP tool | Use it for |
|---|---|
| `tpu_read_file` | reading text files (UTF-8, UTF-16, Windows-1252, Shift-JIS, …) |
| `tpu_read_head` / `tpu_read_tail` | first/last N lines or bytes |
| `tpu_read_file_binary` | inspecting raw bytes of binary files |
| `tpu_read_file_escaped` | reading text as a single 7-bit-clean escaped line |
| `tpu_create_file` | creating a NEW file — fails if the path already exists |
| `tpu_write_file` | replacing an existing text file's full contents |
| `tpu_append_file` | appending text to an existing file |
| `tpu_replace_in_file` | literal (default) or regex substitution — pass `regex: true` to opt into regex matching; a run that matches nothing is an error; pass `ops: […]` for several substitutions in one atomic write |
| `tpu_edit_file` | targeted insert/delete/splice at known line numbers |
| `tpu_validate_file` | pre-flight assertion that a file is in the expected state |
| `tpu_count_file` | line / word / char / byte / pattern counts, plus `line_ending` and exact `lf_count` / `crlf_count` / `cr_count` |
| `tpu_find` | encoding-aware grep across files and globs (pass `glob` to filter a directory walk, e.g. `path: "DIR", glob: "**/*.ndjson"`) |
| `tpu_copy_file` | copy a file or recursively copy a tree (resilient: per-entry warnings, never aborts mid-walk by default) |
| `tpu_render_file` | populate a file from a `{{TOKEN}}` template |
| `tpu_stat_file` | verify a write actually persisted (mtime / size) |
| `tpu_doctor` | conformance check: scan files/dirs/globs for mojibake, encoding damage, **and line-ending mismatches**; repair with `fix: "peel" \| "eol" \| "all"` |
| `tpu_setup` | (re)write this guidance block into the active `copilot-instructions.md` |

### When to use each

- **Reads** — always use `tpu_read_file`. Never use PowerShell `Get-Content`
  for code review or content inspection.
- **Line ranges must start inside the file** — a `lines` argument (on
  `tpu_read_file` or `tpu_read_file_escaped`) whose *start* is past the last
  line is an **error** that reports the file's real line count; it is not
  empty output. End bounds are still clamped, so `lines: "1-9999"` remains
  the safe way to say "from here to EOF". Re-anchor using the line count in
  the error rather than probing with another guess.
- **New files** — use `tpu_create_file`, not `tpu_write_file`. It fails when
  the path already exists, so a mistaken path is reported instead of
  silently destroying whatever was there. Reach for `tpu_write_file` only
  when you intend to replace the contents of a file you know exists.
- **Edits** — prefer `tpu_replace_in_file` (literal matching by default,
  no escaping needed) over `tpu_edit_file` when the target text is unique,
  because line numbers can shift between reads. Use `tpu_edit_file` when
  you have just read the file and know exact line offsets. Every text
  payload — `content`, `replacement`, an op's `data` — is written
  **verbatim**: backslashes are never collapsed, so no tpu tool needs
  pre-doubled escapes. (`tpu_replace_in_file` accepts an opt-in
  `expand_escapes: true` for callers that deliberately double-escape.)
- **Line-mode `tpu_edit_file` edits are targeted** — an insert / delete /
  splice at line numbers rewrites only the lines it touches. Every other
  line is preserved byte-for-byte, including its original terminator and
  encoding. Only the edited or inserted lines get a freshly synthesised
  terminator, matching the file's detected convention (or an explicit
  `line_ending`). (One exception: appending past an unterminated final line
  first terminates that line, so the appended text cannot weld onto it.)
  Consequently a `line_ending` on `tpu_edit_file` re-ends only the touched
  lines — it does **not** convert the whole file. To normalise every line
  ending, rewrite the file with `tpu_write_file` or run `tpu_replace_in_file`
  with a `line_ending` (both operate on the whole file).
- **Verifying line endings — use `tpu_count_file` or `tpu_doctor`, never a
  shell.** Two different questions, two different tools:
  - *"What is actually in the file?"* → `tpu_count_file`. Its stats always
    include `lf_count`, `crlf_count`, and `cr_count` — positive byte
    evidence that a write landed as intended. `line_ending` is `LF`, `CRLF`,
    `CR`, or **`MIXED`** when more than one convention is present; a mixed
    file is never reported as its dominant convention, because a single
    stray CRLF in an LF-only repo is a file git rejects at commit.
  - *"Is this file committable?"* → `tpu_doctor`. It judges the file against
    the repository's own policy (`.gitattributes` / `core.autocrlf` /
    `core.eol`) and answers with a top-level `verdict` of `"clean"` or
    `"issues"`. `verdict` and `total_issues` describe what the scan *found*,
    so a `fix` run that repaired everything still reports `"issues"`; what is
    left afterwards is `total_unresolved`, plus a per-file `unresolved`. Add
    `quiet: true` for just the verdict and the offending
    paths. This is the conformance check — reach for it before falling back
    to PowerShell `ReadAllBytes` and counting `0x0D`. (From a shell, the
    same report comes from `tpu doctor <path> --message-format=json` as one
    structured NDJSON record; the older `--format=json` still prints a
    standalone pretty document. `tpu doctor` exits 1 when anything is
    unresolved and 0 when nothing is, so a `--fix` run that repairs
    everything it finds succeeds and the command works as a commit gate.)
- **`eol_warning` on a mutating tool means the write left a non-conforming
  file** — `tpu_create_file` / `tpu_write_file` / `tpu_replace_in_file` /
  `tpu_edit_file` / `tpu_append_file` / `tpu_render_file` add an
  `eol_warning` field to the status trailer when the resulting file's line
  endings disagree with git's expectation for that path, or are mixed with no
  policy to judge them by.
  The write still succeeded; on a real write, a plain `"status":"success"`
  without this field is what means "committable". Preview modes
  (`count: true`, `dry_run: true`, `tpu_append_file` with `diff: true`)
  write nothing, so they never carry it. For a replace preview, read
  `line_endings.after` instead; `tpu_append_file`'s preview answers only
  `would_write`, so ask `tpu_count_file` or `tpu_doctor` about its line
  endings.
  Repair with `tpu_doctor` and `fix: "eol"`.
- **Several substitutions to one file — use `ops`, never a shell loop.**
  `tpu_replace_in_file` takes an `ops` array instead of a top-level
  `pattern`/`replacement`; each entry accepts the same fields plus an
  optional `label`. This is the tool for a bulk rename or a mechanical
  refactor, and it is the case that most often tempts an agent into
  PowerShell — don't go there. The ops run **in order against the evolving
  buffer** (a later op sees earlier ops' output) and land as **one** atomic
  write: one `.bak`, one mtime bump, one `content_version` — when the
  resulting bytes differ, which an identity substitution's do not. If any op
  matches
  zero times without its own `allow_no_match: true`, the **whole batch** is
  refused and the file is left untouched, so a batch can never leave a
  half-transformed file. `count: true` and `dry_run: true` are exempt — they
  write nothing, so zero is a legitimate answer and the preview succeeds with
  the tally. That refusal is *unconditional* for a real write:
  unlike the single-op form, a `line_ending` override does **not** exempt it,
  because converting terminators says nothing about whether your patterns
  were right. The response carries the per-op tally as
  `"ops":[{"label":"…","count":N},…]` next to the total `count` — that tally
  *is* the verification, so no follow-up probe is needed. There is no
  changed-region echo in batch mode (a region's line numbers would refer to
  an intermediate buffer); pass `diff: true` for a whole-file old/new diff.
  The CLI takes the identical array: `tpu replace FILE --ops ops.json`
  (or `--ops -` for stdin) parses the same objects with the same code, so a
  batch moves between the two unchanged. Every per-op field (`label`,
  `pattern`, `replacement`, `regex`, `multiline`, `allow_no_match`, the
  `*_format` channels) is **rejected** at the top level alongside `ops`,
  because applying none of them silently is how a batch quietly does the
  wrong thing.
- **Verifying a replace — read the response, don't re-read the file.**
  Every `tpu_replace_in_file` reply (including `count: true` and
  `dry_run: true`) carries `"line_endings"` with a **before and after**
  terminator census: `{"uniformity":"uniform"|"mixed"|"none","dominant":…,
  "line_count":N,"lf":N,"crlf":N,"cr":N}` for each. It costs nothing — the
  census is taken during the decode the substitution already performs.
  - `"normalized": true` means the write collapsed a **mixed** file onto one
    convention. A replace rewrites the *whole* file, so every terminator is
    re-emitted in the target convention even when your substitution had
    nothing to do with line endings. This is the single most common source
    of "why did my diff touch every line" — now it says so.
  - `count: true` also reports `"would_write"` — whether the bytes on disk
    would actually change. Ask it rather than inferring from `count`: a
    `line_ending` override rewrites a file with zero substitutions, and an
    identity substitution changes nothing with a non-zero one.
  - Pass `changed_line_details: true` for `changed_line_details`: one entry
    per differing line with `old_line`/`new_line` (null for a pure
    insertion/deletion) and `old_text`/`new_text`. Positions name real lines
    of the real before/after files, so this works for a batch too. Capped by
    `changed_line_details_max` (default 50), with
    `changed_line_details_truncated: true` when the cap is hit. Asking for it
    and finding nothing gives an empty array, not a missing key. Batch mode
    turns this on by default because it has no changed-region echo — unless
    `diff: true` asked for a whole-file diff instead, and never under
    `count: true`, which performs no substitution and so has nothing to
    image; asking for details there explicitly is an error, not an empty
    answer. (Not to be confused with the integer `changed_lines` already in
    the trailer, which is only the size estimate that gates the echo.)
- **A match is not a write** — the status trailer carries `"wrote"`. An
  identity substitution matches and reports a non-zero `count`, but produces
  byte-identical output, which the write path skips: no `.bak`, no mtime
  bump. The changed-region echo still shows the match (it describes what
  matched, not what changed), so `wrote: false` is what tells you the file is
  untouched.
- **An empty literal pattern is refused** — it would match at every byte
  position and splice the replacement between every character of the file,
  reporting a large and entirely plausible count. Pass `regex: true` if an
  empty pattern is genuinely what you want. Patterns are matched against an
  LF-normalised view and are themselves normalised, so a pattern containing a
  literal CRLF matches a CRLF file — you never need to account for line
  endings in a pattern.
- **A replace that matches nothing is an error** — `tpu_replace_in_file`
  returns `{"status":"error"}` when `pattern` matches zero times, and leaves
  the file completely untouched (mtime preserved, no `.bak`). This is
  deliberate: a silent success on a mis-anchored pattern is
  indistinguishable from a real edit. Re-read the file and re-anchor the
  pattern instead of retrying blind. Pass `allow_no_match: true` only for a
  genuinely idempotent re-run — the response then carries `count: 0` and a
  `warning`. `count: true` and `dry_run: true` are exempt (zero is a
  legitimate answer for an introspection mode), as is a `line_ending`
  override, which rewrites the file even with zero substitutions. (That
  override exemption is single-op only — see the `ops` bullet above.) A real
  write always reports `count`, so no follow-up `count: true` call is needed
  to confirm how many substitutions landed.
- **Writes that should be guarded** — pass `validate: [{ "selector":
  "line-contains:N", "value": "..." }]` to refuse the write if the file is
  not in the expected state.
- **Globs / recursion** — `tpu_find` and `tpu_copy_file` accept glob
  patterns and tolerate inaccessible directories by emitting warning
  records (configurable via the `on_error` argument). To search a directory
  tree with `tpu_find`, pass the directory as `path` and the filename
  pattern as `glob` (e.g. `path: "q:/src/foo/.scratch", glob: "**/*.ndjson"`)
  — this is the `find DIR -name PAT` shape and is the only way to recurse
  into an absolute directory.
- **Dependency-free templating** — `tpu_render_file` substitutes
  `{{NAME}}`-style tokens. Use `\{{` to emit literal braces.

### Escape-sequence hazard (JSON transport)

MCP arguments travel as JSON strings. In JSON, `\n` **is** a real newline —
a literal backslash-n in the file requires `\\n` on the wire. Under-escaping
is easy for an agent to do because it "sees" the target source text, not
the JSON transport, and the damage is invisible: the string is already
decoded to a real newline *before* `tpu_write_file` / `tpu_append_file` /
`tpu_replace_in_file` / `tpu_edit_file` ever runs, so no flag on the tool
call can distinguish "intended literal `\n`" from "intended real newline" —
no server-side option can undo a JSON decode that already happened.

tpu does not add a second layer of its own: every text payload is written
verbatim, so a correctly JSON-escaped string always lands byte-for-byte.
(`tpu_replace_in_file`'s `expand_escapes: true` is the sole exception, and
it is opt-in — leave it off unless you deliberately double-escaped.) The
residual hazard is purely in getting the JSON escaping right.

**The fix**: when a payload (`content`, `pattern`, `replacement`, or an
edit op's `data`) contains backslash escapes, embedded quotes, or anything
not certain to be JSON-escaped correctly, set the matching `*_format`
argument (`content_format`, `pattern_format`, `replacement_format`, or an
op's `data_format`) to `"base64"` and send the exact bytes base64-encoded.
Base64's alphabet has no backslashes, so there is no escaping decision to
get wrong — this makes the whole class of bug impossible rather than just
less likely. `"hex"` works the same way; avoid `"encoded"` for this purpose
since it is itself a backslash-escape codec and re-introduces the hazard.

**Safety net**: `tpu_replace_in_file` also echoes a compact changed-region
preview of every small real write by default (no `diff:true` needed — see
`echo_max_lines`), so a corruption like this is visible immediately in the
same turn instead of requiring a follow-up read. This echo is cheap
regardless of file size (it never clones the whole file); pass `diff:true`
for a full old/new unified diff instead.

### Concurrent edits — don't silently clobber (`content_version` / `if_match`)

Copilot may issue several tool calls against the same file in quick
succession. A blind `tpu_write_file` (or a `tpu_edit_file` at line numbers)
whose payload was computed from an earlier read can silently overwrite an
edit that landed in between — a lost update.

Every read (`tpu_read_file`, `tpu_read_head`, `tpu_read_tail`, `tpu_read_file_escaped`)
reports a `"content_version"` on its invocation-header line (a content digest
that changes whenever the file's bytes change), and every successful write
stamp reports the new `"content_version"`. A read MAY omit the token when the
file changed while it was being read (so it can't be guaranteed to describe
the bytes returned) — treat a missing token as "re-read before relying on a
version".

When you mutate a file based on content you previously read, pass that token
as `if_match` on `tpu_write_file` / `tpu_edit_file` / `tpu_replace_in_file` /
`tpu_append_file`. If the file changed since you read it, the call is
REFUSED with `{"status":"conflict",...}` (surfaced as an MCP error,
`isError: true`) and the file is left unchanged, instead of clobbering the
other edit; then re-read, rebuild your change against the current content,
and retry with the new `content_version`.

Prefer a narrow `tpu_replace_in_file` over a full-file `tpu_write_file` when
you can: a replace operates on the file's current bytes, so it is far less
prone to lost updates in the first place.

### Tool output format

Every tool response uses a **mixed format**: a JSON invocation header,
a content-type-dependent body, and (for most tools) a JSON status trailer.
Not every line is JSON — read tools and `find` return raw content
between the header and trailer.

- **Line 1** — invocation header:
  `{"reason":"x-tpu-mcp-invocation","tool":"tpu_NAME","args":{...}}`
  Large `content`/`replacement`/`template` fields appear as `"<N bytes>"` placeholders.
- **Mutating tools** (write, replace, edit, append) — normal write:
  `{"status":"success","file":"...","mtime_epoch_ms":N,"size":N,"content_version":"..."}`
  (`content_version` is the digest of the just-written file — pass it as
  `if_match` on your next edit of this file; see "Concurrent edits" above).
  A `tpu_replace_in_file` whose `pattern` matched zero times is instead an
  error trailer naming the count, with the file left untouched — see
  "A replace that matches nothing is an error" above.
  Any of the six text-writing tools (`tpu_create_file` / `tpu_write_file` /
  `tpu_replace_in_file` / `tpu_edit_file` / `tpu_append_file` /
  `tpu_render_file`) may also carry `"eol_warning":"..."` — the write landed,
  but the resulting file's line endings do not conform (see the `eol_warning`
  bullet above).
  A `tpu_replace_in_file` batch (`ops`) additionally reports
  `"ops":[{"label":…,"count":N},…]` in every mode — real write, `count:true`,
  and `dry_run:true` alike — and omits the changed-region echo.
  Every `tpu_replace_in_file` reply also carries `"line_endings"`
  (before/after census plus `normalized`), and `"changed_line_details"` when
  asked for — see "Verifying a replace" above.
  Preview modes do not stamp the file and return a reduced trailer:
  `diff:true` adds unified diff lines before the status (full stamp still present for write/replace/edit).
  `dry_run:true` (replace only): optional diff lines, then `{"status":"success","changed":true|false,"would_write":true|false}`
  — the two are the same value; prefer `would_write`, which is the name every
  preview mode uses.
  `count:true` (replace only): `{"status":"success","count":N,"would_write":true|false}`.
  `append diff:true`: diff lines when the text differs, then `{"status":"success","file":"...","changed":true|false,"would_write":true|false}`
  — both are the byte-level answer, so a `line_ending` override that rewrites
  every terminator reports true even though the diff (taken in LF space) is
  empty.
  `tpu_replace_in_file` on a real write additionally reports `"changed_lines":N` in the status —
  the sum, over every match, of `(old span line count) + (new text line count)`; this is a
  cheap per-match total, NOT a deduplicated count of unique file lines, so two matches on the
  same line each contribute their own share and can push N above the file's actual line count —
  and, as long as N is at most `echo_max_lines` (default 5), automatically prepends a
  compact changed-region preview (unified-diff-style hunk headers, new lines only — no
  full-file diff) even without `diff:true`; a larger change instead adds
  `"diff_omitted":true` (pass `diff:true` for a full old/new unified diff regardless of size).
- **Structured tools** (count_file, stat_file, copy_file, render_file,
  setup+target, doctor) — result line
  `{"reason":"x-tpu-mcp-result",...}` followed by `{"status":"success"}`.
- **Read tools** (tpu_read_file, tpu_read_head, tpu_read_tail, tpu_read_file_escaped) — header then raw content; no JSON trailer on success.
  The header line usually carries a `"content_version"` token for this file (see "Concurrent edits" above); pass it as `if_match` when you later edit the file. It may be absent if the file changed mid-read — re-read to get a usable token.
  **Exception** — `tpu_read_file_binary` with a non-empty `hash` arg acts like a structured tool:
  `{"reason":"x-tpu-mcp-result","encoding":"bytes-base64","content":"<base64>","hashes":[...]}` followed by `{"status":"success"}`.
  Without `hash`, `tpu_read_file_binary` returns header + 7-bit-clean escaped bytes (no trailer).
- **Find tool** (find) — header, then matching lines as plain text, then `{"status":"success","warnings":[...]}` trailer.
- **Errors** — `{"status":"error","message":"..."}` as the final line;
  `isError: true` in the MCP wrapper.

### When a file looks corrupted (mojibake)

Symptoms: `Ã©` where `é` should be, `â€"` where `—` should be, `â"€` instead
of `─`, stray `Â ` before numbers, `ð\u009f...` blobs instead of emoji.
This is *mojibake* — text that was decoded in the wrong encoding and then
re-encoded as UTF-8. It is almost always caused by a non-tpu writer round-
tripping the file through the OS code page (PowerShell `Get-Content` /
`Set-Content` / `Out-File` / `>` / `Add-Content`, a misconfigured editor,
a generator that assumed ASCII).

Workflow:

1. **Diagnose**: call `tpu_doctor` with the suspect file (or the
   surrounding directory / glob). It returns a JSON report listing every
   flagged file, its detected encoding, per-pattern match counts, exact
   line/column locations, and whether a one-layer "peel" repair would
   strictly improve the file (`peel_suggested: true`).
2. **Identify the offender**: when a file is corrupted in a git repo, run
   `git log -p -- <file>` (or `git blame -- <file>`) to find the
   introducing commit. The commit reveals which tool wrote the damage so
   you can stop the leak at the source rather than only repairing
   downstream.
3. **Repair (conservative)**: call `tpu_doctor` again with
   `fix: "peel"`. Only files whose peel produces *strictly fewer* mojibake
   matches are rewritten; the prior content is preserved at `<file>.bak`.
   Re-run `tpu_doctor` after the repair to confirm the report is clean.
   When a flagged file's peel is declined (`peel_suggested: false` despite
   having `mojibake_matches`), `peel_declined_reason` explains why — e.g.
   the peel would itself produce invalid UTF-8, or would not reduce (or
   would increase) the match count, typically because other legitimate
   multi-byte UTF-8 text elsewhere in the file would be corrupted by a
   whole-file reverse-decode. That file needs manual repair; don't treat
   the unset `repaired` flag as a tool failure.
4. **Don't paper over it**: if a file legitimately contains mojibake
   digraphs (test fixtures, regex sources, documentation about mojibake),
   add the line `encoding-check: allow-mojibake` (typically inside a
   comment) — `tpu_doctor` and the write-time guard will honour the
   opt-out (it is never counted toward `total_issues` or exit-code
   failures). This is **not** silent, though: if the file would otherwise
   have been flagged, it still appears in `tpu_doctor`'s `files` array with
   `mojibake_marker_suppressed` (and/or `replacement_char_marker_suppressed`)
   set to the count that was hidden, and the top-level
   `total_marker_suppressed` reports how many files that applied to across
   the whole scan. The marker covers mojibake and replacement-character
   diagnostics only — a git line-ending mismatch is a separate concern and
   is still reported for a marked file. A file with the marker but genuinely
   nothing to suppress
   is omitted entirely, same as any other clean file. If you see a nonzero
   `total_marker_suppressed`, don't assume the marker was placed
   correctly — a file that merely *discusses* the marker string in prose
   (rather than opting out real, adjacent corruption) will also suppress
   whatever mojibake happens to be nearby, since the marker is a
   whole-file substring match, not scoped to a region.

The write-time guard in `tpu_write_file` / `tpu_append_file` /
`tpu_replace_in_file` / `tpu_edit_file` already refuses to *introduce* new
mojibake (pre-existing damage passes through). If you genuinely intend to
write curated mojibake fixtures, pass `allow_mojibake: true`.

### Git worktree encoding and line endings

For files in a Git worktree, TPU automatically discovers the nearest repository
and resolves `.gitattributes` with gitoxide. `working-tree-encoding` controls
decoding, strict re-encoding, and BOM policy. `text`/`eol` attributes and
`core.autocrlf` / `core.eol` determine a definite working-tree line ending when
Git provides one.

Discovery and open repository handles are cached per tool operation, so a glob
rooted above multiple repositories opens each worktree at most once. Caches are
refreshed between operations so repository and configuration changes are
observed. `git_root` remains a legacy read-advisory hint; encoding and mutation
policy use nearest-repository discovery.

Policy is resolved independently for each path as it is processed. Multi-file
operations do not snapshot or lock `.gitattributes` for their full duration.
Concurrent attribute edits may therefore cause paths resolved before and after
the edit to use different policies; resolution failures remain per-file errors
rather than silent defaults.

1. **Read**: text tools decode using `working-tree-encoding`;
   `tpu_read_file`, `tpu_read_head`, and `tpu_read_tail` also prefix a `note:`
   when on-disk endings disagree with Git's expectation.
2. **Write**: text mutations strictly re-encode in the declared worktree
   encoding, enforce its BOM rules, and normalise all endings when Git supplies
   a definite convention. An explicit `line_ending` takes precedence.
   **Exception:** line-mode `tpu_edit_file` is *targeted* — it rewrites only the
   lines it touches and preserves every other line's original terminator, so it
   re-ends only edited / inserted / appended lines rather than the whole file
   (see the `tpu_edit_file` bullet above). It still enforces the required BOM.
3. **Report / repair with doctor**: `tpu_doctor` lists mismatches with an
   `eol_mismatch` object, whose `actual` names the *offending* ending rather
   than the dominant one — a mostly-LF file with a few CRLFs is still flagged.
   `fix: "eol"` normalises endings only; `fix: "all"` also peels mojibake.
   Repairs are atomic, retain a `<file>.bak`, and support UTF-16. Each file
   reports `mojibake_repaired`, `eol_repaired`, and their union `any_repaired`,
   so a successful EOL fix is never misread as a no-op from the legacy
   mojibake-only `repaired` field.

### File encoding

When you must fall back to PowerShell, never round-trip non-ASCII files
through `Get-Content` / `Set-Content` — read and write via
`[System.IO.File]::ReadAllBytes` / `WriteAllBytes` and validate with
`tools/check-encoding.ps1` afterwards.
<!-- tpu-mcp:setup:end -->
