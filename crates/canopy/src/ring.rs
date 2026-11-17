// Copyright (c) Michael Grier.

//! The ring: queue pairs and the doorbells that wake their consumers, written once over a
//! platform signal (DESIGN-NOTES "The ring: a portable protocol over one platform primitive,
//! behind a contract spec").
//!
//! The one platform primitive is acadia's [`Signal`](acadia::Signal); everything that makes a
//! doorbell correct is here, generic over it, so a doorbell over a Windows event, a Linux
//! `eventfd` and a condition variable is the same code, monomorphized.
//!
//! [`Session`] builds an in-process asynchronous submission/completion pair over the store. A
//! producer pushes commands without waiting, then rings once for a batch. The engine executes
//! ready pairs in round-robin order, parks a command that cannot acquire writer exclusion, and
//! publishes one completion. The queue ends are `Send` but not `Sync`; mutation takes `&mut self`.
//!
//! The public model has no queue or worker state. Accepted requests correlate only inside their
//! pair, completion capacity is reserved at submission, each pair preserves request order, and
//! at most one completion is delivered. If the pair is lost, no completion is manufactured; the
//! request's effect remains uncertain even when a later read happens to observe an applied body.
//! These promises are named `SubmissionOutcomesCoherent`, `CompletionCapacityReserved`,
//! `PerPairCompletionOrder`, `AtMostOneDelivery`, `LostCompletionIsUnknown`, and
//! `RecoveryReportsUncertainEffect` in `ClientContract.tla`.

mod doorbell;
mod session;
pub(crate) mod sync;

pub use acadia::Watch;
pub use doorbell::Doorbell;
pub use session::{
    Command, CommandValue, Completion, CompletionQueue, RequestId, Session, SessionError,
    SnapshotCommand, SnapshotValue, SubmissionQueue, SubmissionRefusal, SubmitError,
};
