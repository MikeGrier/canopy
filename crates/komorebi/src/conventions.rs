// Copyright (c) Michael Grier.

//! Canopy's event and field vocabulary for `tracing` (LOG-2, D57).
//!
//! `tracing` is only the mechanism; the vocabulary is ours (design autonomy).
//! Call sites emit through the `tracing` macros directly, passing a `target:`
//! drawn from [`targets`] and structured fields whose names match the constants
//! in [`fields`], with well-known field values drawn from [`outcomes`],
//! [`phases`], and [`results`]. This module is deliberately declarative: a
//! shared set of `&'static str` names, not a facade over the macros.
//!
//! # Why constants for names `tracing` wants as literals
//!
//! `tracing` requires a field *name* to be a literal identifier at the macro
//! call site -- a `const` cannot be substituted there -- so a call reproduces
//! the field name literally (e.g. `event = ...`). The constants in [`fields`]
//! are the single canonical spelling of each name: they are what a reviewer
//! checks call sites against, and -- because they are also the string keys a
//! capturing subscriber sees -- what tests match on when asserting *what was
//! observed*. The `target:` argument, by contrast, does accept a `const`
//! expression, so call sites pass a [`targets`] constant directly.
//!
//! # Example
//!
//! ```
//! use komorebi::conventions::{fields, outcomes, targets};
//! use tracing::info;
//!
//! // `target:` takes the constant directly; field names are written as
//! // literals matching the `fields` constants (tracing needs a literal there),
//! // and a well-known field value comes from `outcomes`.
//! info!(target: targets::LIFECYCLE, event = "daemon_start", outcome = outcomes::COMPLETED);
//! let _ = (fields::EVENT, fields::OUTCOME);
//! ```

/// `tracing` `target:` names for Canopy's event categories.
///
/// Each category is a distinct target so filtering (the [`crate::LogConfig`]
/// `EnvFilter` directives) can raise or lower verbosity per category
/// independently -- e.g. `canopy::recovery=debug` without touching the rest.
pub mod targets {
    /// Daemon and job lifecycle transitions -- the small monotone status
    /// (started/stopped and the steps between) that says the system, or a job,
    /// is making forward progress (D5/D28).
    pub const LIFECYCLE: &str = "canopy::lifecycle";

    /// The startup recovery pass: opening and validating the Shire, recovering
    /// the shire-level singletons, replaying the commit log, rebuilding volatile
    /// state, and re-attaching orphaned children (D46).
    pub const RECOVERY: &str = "canopy::recovery";

    /// Verify-read and the verify/repair scrub: confirming copies agree and
    /// rebuilding missing or erroneous copies from the majority (D40).
    pub const VERIFY_REPAIR: &str = "canopy::verify_repair";

    /// The control-endpoint rendezvous (D52): publishing, discovering, and
    /// withdrawing a daemon's discovery file. A distinct target so
    /// client-side discovery can be filtered apart from daemon lifecycle.
    pub const DISCOVERY: &str = "canopy::discovery";

    /// The inbound gRPC adapter (`canopy-ring-adapter`, D51): per-call transport mechanics
    /// -- request marshalling, error mapping, and the D29 log-exfil follow stream
    /// (open, per-event projection, backpressure, client disconnect). A distinct
    /// target so noisy transport tracing can be raised (e.g. `canopy::rpc=trace`)
    /// without touching daemon or job lifecycle.
    pub const RPC: &str = "canopy::rpc";

    /// The engine's file-backed output-stream pumps (`D91`/`D93`): per-channel
    /// pump lifecycle (`pump_start`/`pump_end`), the stranded-cursor no-progress
    /// guard, and the `counters_snapshot` post-mortem tallies. A distinct target
    /// so a streaming wedge can be traced (e.g. `canopy::stream=debug`) without
    /// touching daemon, job, or transport tracing.
    pub const STREAM: &str = "canopy::stream";
}

/// Structured field-name keys shared across the event categories.
///
/// A field is written at the call site as a literal whose spelling matches the
/// constant here; the constant is the canonical spelling and the key a
/// capturing subscriber (and thus a test) looks up.
pub mod fields {
    /// Short name of the specific event within its target (e.g. `"daemon_start"`,
    /// `"replay_log"`). Distinguishes events that share a target.
    pub const EVENT: &str = "event";

    /// Terminal disposition of a lifecycle step; a value from [`super::outcomes`].
    pub const OUTCOME: &str = "outcome";

    /// Which recovery phase an event belongs to; a value from [`super::phases`].
    pub const PHASE: &str = "phase";

    /// The result of a verify/repair action; a value from [`super::results`].
    pub const RESULT: &str = "result";

    /// Identity (or path) of the Shire -- the store root -- an event concerns.
    pub const SHIRE: &str = "shire";

    /// Ticket / job identifier an event concerns.
    pub const TICKET: &str = "ticket";

    /// Storage partition an event concerns, in quorum contexts.
    pub const PARTITION: &str = "partition";

    /// Generation number of the durable record an event concerns (D26).
    pub const GENERATION: &str = "generation";

    /// Filesystem path an event concerns, when one is relevant.
    pub const PATH: &str = "path";

    /// Network address (`host:port`) an event concerns -- e.g. the control
    /// endpoint a discovery event published or resolved (D44/D52).
    pub const ADDRESS: &str = "address";

    /// Number of good copies found, in a quorum/verify context.
    pub const COPIES_PRESENT: &str = "copies_present";

    /// Number of agreeing copies required to reach quorum (D40).
    pub const QUORUM: &str = "quorum";

    /// Human-readable reason accompanying a failure or a skip.
    pub const REASON: &str = "reason";

    /// Elapsed wall-clock time for a completed step, in milliseconds.
    pub const ELAPSED_MS: &str = "elapsed_ms";

    /// Output channel (`stdout`, `stderr`, or a named channel) a stream event
    /// concerns (D48).
    pub const CHANNEL: &str = "channel";

    /// Absolute byte offset within a channel an event concerns (D48).
    pub const OFFSET: &str = "offset";

    /// Byte count an event concerns -- a chunk's length or a channel's final
    /// length (D48).
    pub const LENGTH: &str = "length";

    /// Cumulative bytes a stream pump has forwarded to the conduit for a channel
    /// by the time an event is emitted (`D93`).
    pub const FORWARDED: &str = "forwarded";
}

/// Well-known values for the [`fields::OUTCOME`] field.
pub mod outcomes {
    /// A step has begun.
    pub const STARTED: &str = "started";

    /// A step finished successfully.
    pub const COMPLETED: &str = "completed";

    /// A step ended in failure; pair with [`super::fields::REASON`].
    pub const FAILED: &str = "failed";

    /// A step was intentionally not performed (e.g. an O(1) recovery skip on a
    /// cleanly-quiesced store); pair with [`super::fields::REASON`].
    pub const SKIPPED: &str = "skipped";
}

/// Well-known values for the [`fields::PHASE`] field, one per ordered step of
/// the D46 startup recovery pass.
pub mod phases {
    /// Open and validate the Shire, refusing to start if it is absent (D40/D46).
    pub const OPEN_SHIRE: &str = "open_shire";

    /// Quorum-recover the shire-level singletons (D39/D36).
    pub const RECOVER_SINGLETONS: &str = "recover_singletons";

    /// Replay the commit log to roll interrupted transactions forward or back
    /// (D53).
    pub const REPLAY_LOG: &str = "replay_log";

    /// Rebuild volatile state (repair queue, monitor, high-water) after replay.
    pub const REBUILD_VOLATILE: &str = "rebuild_volatile";
}

/// Well-known values for the [`fields::RESULT`] field of a verify/repair action.
pub mod results {
    /// Copies agreed; nothing to repair (D40 verify-read).
    pub const VERIFIED: &str = "verified";

    /// A missing or erroneous copy was rebuilt from the majority (D40).
    pub const REPAIRED: &str = "repaired";

    /// No quorum was reachable, so the record could not be repaired (D40).
    pub const UNREPAIRABLE: &str = "unrepairable";
}

/// Well-known values for the [`fields::REASON`] field of a `pump_end` stream
/// event (`D93`): why a file-backed output pump stopped forwarding a channel.
pub mod stream_reasons {
    /// The channel ended and the pump drained it to its final length, then sent
    /// the `End` marker -- the ordinary, successful terminus.
    pub const COMPLETED: &str = "completed";

    /// The outbound relay receiver was gone, so there is no consumer left to
    /// forward to; the pump stops dropping the rest of the stream.
    pub const RELAY_DROPPED: &str = "relay_dropped";

    /// A spill-file read failed; the pump cannot continue tailing the channel.
    pub const READ_ERROR: &str = "read_error";

    /// The pump's cursor was stranded below the retained window (a `D89` release
    /// advanced the low-water past it), so `wait_past` kept reporting progress
    /// the pump could never forward -- a no-progress guard trip, made terminal
    /// rather than a busy-spin (`D93`).
    pub const STRANDED: &str = "stranded";
}
