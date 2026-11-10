# komorebi

Logging setup and test-time event capture for the Canopy workspace. Built on
`tracing` and `tracing-subscriber`.

**Internal crate, not published.** `canopy` uses it only as a dev-dependency.

## What it provides

- **`LogConfig` / `init`** -- builds and installs the process-wide `tracing`
  subscriber. Defaults: level `info`, overridable through `RUST_LOG`, written to
  stderr in the full format with ANSI colors. Builders change the default
  directives, the environment variable consulted (an empty name disables the
  override), the target (`LogTarget::Stdout` / `Stderr`), the format
  (`LogFormat::Full` / `Compact` / `Pretty`), and ANSI output. An unparseable
  directive is dropped rather than failing startup, and installing twice returns an
  `InitError` instead of panicking.
- **`conventions`** -- the shared vocabulary for events: `targets`, `fields`, and
  well-known values in `outcomes`, `phases`, and `results`. Call sites use the
  `tracing` macros directly with these names; `tracing` is the mechanism, the
  vocabulary is ours.
- **`capture`** -- records the events a piece of code emits so a test can assert on
  what was observed, not only on the final state. `with_capture` runs a closure and
  returns its events alongside its result; `install` returns a handle and guard for
  longer scopes.

## Configuring logging

```rust
use komorebi::{LogConfig, LogFormat, LogTarget};

LogConfig::default()
    .with_default_directives("info,canopy=debug")
    .with_target(LogTarget::Stderr)
    .with_format(LogFormat::Compact)
    .with_ansi(false)
    .install()
    .expect("no subscriber installed yet");
```

## Capturing events in a test

```rust
use komorebi::capture::with_capture;
use komorebi::conventions::{outcomes, targets};
use tracing::info;

let (events, ()) = with_capture(|| {
    info!(target: targets::LIFECYCLE, event = "daemon_start", outcome = outcomes::STARTED);
});

assert_eq!(events.len(), 1);
assert_eq!(events[0].target(), targets::LIFECYCLE);
assert_eq!(events[0].field("outcome"), Some("started"));
```

Capture installs a **thread-local** subscriber, so it is deterministic under
parallel tests and can be installed many times per process. Only events emitted on
the capturing thread are recorded, and no filtering is applied, so `RUST_LOG` does
not affect what a test sees.
