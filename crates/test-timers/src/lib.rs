// Copyright (c) Michael Grier.

//! The one way test code reaches a timer (`D484`, `D485`).
//!
//! `clippy.toml` disallows the timer entry points, so that a production call has to say, in an
//! `expect`, whether it is transient, process-wide, or a known violation of `D120` and `D483`. Test
//! code is not governed by that rule -- a test's waits are transient by nature -- but it does not get
//! a lint-wide exemption either, because that would also lift the transport, runtime and
//! call-metadata rules from it. So tests call these wrappers instead, and this crate is the one place
//! that carries the annotation, as an `expect`, so that it fails the build if the lint ever stops firing here.
//!
//! The modules mirror the paths they stand in for, so moving a test onto them is a path change:
//! `tokio::time::sleep` becomes [`tokio::sleep`], `std::thread::sleep` becomes [`thread::sleep`],
//! and a receiver's `recv_timeout` becomes [`mpsc::recv_timeout`].
//!
//! **Take this crate only as a dev-dependency.** A production crate that depended on it normally
//! would have an unannotated way around the rule; `tests/dev_only.rs` fails the build if one does.

/// The `tokio` timers.
pub mod tokio {
    use std::time::Duration;

    /// [`tokio::time::sleep`], for a test.
    ///
    /// Returns the future itself rather than awaiting it, so it can be raced in a `select!`.
    #[expect(
        clippy::disallowed_methods,
        reason = "test support: a test's wait is transient, which D120 and D483 do not govern (D485)"
    )]
    pub fn sleep(duration: Duration) -> ::tokio::time::Sleep {
        ::tokio::time::sleep(duration)
    }

    /// [`tokio::time::interval`], for a test.
    #[expect(
        clippy::disallowed_methods,
        reason = "test support: a test's wait is transient, which D120 and D483 do not govern (D485)"
    )]
    pub fn interval(period: Duration) -> ::tokio::time::Interval {
        ::tokio::time::interval(period)
    }
}

/// The thread timers.
pub mod thread {
    use std::time::Duration;

    /// [`std::thread::sleep`], for a test.
    #[expect(
        clippy::disallowed_methods,
        reason = "test support: a test's wait is transient, which D120 and D483 do not govern (D485)"
    )]
    pub fn sleep(duration: Duration) {
        std::thread::sleep(duration);
    }

    /// [`std::thread::park_timeout`], for a test.
    #[expect(
        clippy::disallowed_methods,
        reason = "test support: a test's wait is transient, which D120 and D483 do not govern (D485)"
    )]
    pub fn park_timeout(duration: Duration) {
        std::thread::park_timeout(duration);
    }
}

/// The channel timers.
pub mod mpsc {
    use std::sync::mpsc::{Receiver, RecvTimeoutError};
    use std::time::Duration;

    /// [`Receiver::recv_timeout`], for a test.
    #[expect(
        clippy::disallowed_methods,
        reason = "test support: a test's wait is transient, which D120 and D483 do not govern (D485)"
    )]
    pub fn recv_timeout<T>(
        receiver: &Receiver<T>,
        timeout: Duration,
    ) -> Result<T, RecvTimeoutError> {
        receiver.recv_timeout(timeout)
    }
}

#[cfg(test)]
mod tests;
