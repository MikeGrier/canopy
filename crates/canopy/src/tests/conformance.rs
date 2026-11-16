// Copyright (c) Michael Grier.

//! Replays of the trace corpus, `tests/traces/`, against the store (DESIGN-NOTES "Replaying a
//! trace against the code"). Each spec's bridge drives the code the spec describes -- `Store`, or
//! acadia's coordination -- on `MemFs` through the actions a trace takes, and compares a
//! projection of its live and durable state with the trace's model state after every step.

mod coordination;
mod corpus;
mod creation;
mod pointer_publication;
mod recorder;
mod shared_publication;
