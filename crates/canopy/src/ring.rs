// Copyright (c) Michael Grier.

//! The ring: queue pairs and the doorbells that wake their consumers, written once over a
//! platform signal (DESIGN-NOTES "The ring: a portable protocol over one platform primitive,
//! behind a contract spec").
//!
//! The one platform primitive is acadia's [`Signal`](acadia::Signal); everything that makes a
//! doorbell correct is here, generic over it, so a doorbell over a Windows event, a Linux
//! `eventfd` and a condition variable is the same code, monomorphized.

mod doorbell;

pub use doorbell::Doorbell;
