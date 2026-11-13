// Copyright (c) Michael Grier.

//! A seeded discrete-event simulation of contention in canopy's coordination contract, as the
//! quantitative companion to the TLA+ models: they establish what can happen, this predicts how
//! often and how long. The design is in canopy's DESIGN-NOTES, "How long the contract makes a
//! caller wait".
//!
//! Three things are simulated, each in a module of its own:
//! - [`exclusion`]: one exclusive lock -- writer exclusion -- under a choice of discipline for
//!   who is served next, with open, closed and bursty arrivals;
//! - [`snapshot`]: a reader that samples the modification count around a read while writes go
//!   on, and how often it gives up;
//! - [`gate`]: an upgrader that needs the admission gate exclusively, and then every attachment
//!   gone from the tiers, while handles go on attaching.
//!
//! Every run is reproducible from its seed. Times are in seconds.

pub mod baseline;
pub mod dist;
pub mod exclusion;
pub mod gate;
pub mod report;
pub mod rng;
pub mod snapshot;
pub mod stats;

#[cfg(test)]
mod tests;
