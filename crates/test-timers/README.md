# test-timers

The one way test code in this workspace reaches a timer.

**Internal crate, not published. Use it only as a dev-dependency.**

## Why it exists

The workspace [`clippy.toml`](../../clippy.toml) disallows the timer entry points
(`std::thread::sleep`, `std::thread::park_timeout`, `Receiver::recv_timeout`,
`tokio::time::sleep`, `tokio::time::interval`). Production code that genuinely needs
one must say why in an `#[expect(clippy::disallowed_methods, reason = "...")]` at the
call site (D120, D483).

Tests are not governed by that rule -- a test's waits are transient by nature -- but
giving them a lint-wide exemption would also lift every other `disallowed_methods`
rule. Instead, tests call these wrappers, and this crate is the single place that
carries the annotation (D484, D485). The annotation is an `expect`, not an `allow`,
so the build fails if the lint ever stops firing here.

## Usage

The modules mirror the paths they replace, so switching a test over is a path
change:

| Instead of | Use |
|---|---|
| `std::thread::sleep` | `test_timers::thread::sleep` |
| `std::thread::park_timeout` | `test_timers::thread::park_timeout` |
| `receiver.recv_timeout(d)` | `test_timers::mpsc::recv_timeout(&receiver, d)` |
| `tokio::time::sleep` | `test_timers::tokio::sleep` |
| `tokio::time::interval` | `test_timers::tokio::interval` |

```toml
[dev-dependencies]
test-timers = { path = "../test-timers" }
```

`test_timers::tokio::sleep` returns the `Sleep` future without awaiting it, so it can
be raced in a `select!`.

## Enforcement

`tests/dev_only.rs` reads every workspace crate's manifest and fails if
`test-timers` appears anywhere other than a `[dev-dependencies]` table. A normal
dependency would give production code an unannotated way around the timer rule.
