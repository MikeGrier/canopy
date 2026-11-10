// Copyright (c) Michael Grier.

//! Test-capture helper: install a subscriber that records emitted
//! events so a test in any crate can assert on *what was observed* -- the events
//! and their structured fields -- not only the final state.
//!
//! The captured vocabulary is Canopy's own (see [`crate::conventions`]);
//! `tracing` is only the mechanism. A test drives some code, then inspects the
//! [`CapturedEvent`]s it emitted, matching on the [`crate::conventions::targets`]
//! and [`crate::conventions::fields`] names.
//!
//! # Determinism and scope
//!
//! Capture is installed as the **thread-local** `tracing` default (via
//! [`tracing::subscriber::set_default`]), not the process-global one. That is
//! deliberate: a thread-local default is deterministic and does not race with
//! other tests running in parallel, and -- unlike the global default -- it can
//! be installed and torn down many times in one process. The consequence is that
//! only events emitted **on the same thread** while the guard is live are
//! captured; a `tracing` event emitted on a thread the code-under-test spawns is
//! not. Tests that need deterministic capture therefore drive the emitting code
//! on the test thread.
//!
//! The capturing subscriber applies **no filtering** -- it records every event
//! reaching it -- so a test sees exactly what was emitted regardless of any
//! `RUST_LOG`/[`crate::LogConfig`] directives in the environment.
//!
//! # Example
//!
//! ```
//! use komorebi::capture::with_capture;
//! use komorebi::conventions::{outcomes, targets};
//! use tracing::info;
//!
//! let (events, ()) = with_capture(|| {
//!     info!(target: targets::LIFECYCLE, event = "daemon_start", outcome = outcomes::STARTED);
//! });
//!
//! assert_eq!(events.len(), 1);
//! let e = &events[0];
//! assert_eq!(e.target(), targets::LIFECYCLE);
//! assert_eq!(e.field("event"), Some("daemon_start"));
//! assert_eq!(e.field("outcome"), Some("started"));
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::subscriber::DefaultGuard;
use tracing::{Event, Level};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::Registry;

/// A single `tracing` event recorded by the capturing subscriber.
///
/// Field values are captured as strings: a string-valued field keeps its raw
/// text (no surrounding quotes), and any other value keeps its `Debug`
/// rendering (e.g. `42`, `true`). This makes assertions deterministic and
/// independent of formatting/ANSI concerns.
#[derive(Clone, Debug)]
pub struct CapturedEvent {
    target: String,
    level: Level,
    name: String,
    fields: BTreeMap<String, String>,
}

impl CapturedEvent {
    /// The event's `target` (e.g. a [`crate::conventions::targets`] constant).
    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }

    /// The event's severity level.
    #[must_use]
    pub fn level(&self) -> Level {
        self.level
    }

    /// The event's metadata name (its source location identifier).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The captured value of a structured field, if the event carried it.
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }

    /// Whether the event carried a field of this name.
    #[must_use]
    pub fn has_field(&self, name: &str) -> bool {
        self.fields.contains_key(name)
    }

    /// The event's message (the special `message` field), if any.
    #[must_use]
    pub fn message(&self) -> Option<&str> {
        self.field("message")
    }

    /// All captured fields, keyed by name.
    #[must_use]
    pub fn fields(&self) -> &BTreeMap<String, String> {
        &self.fields
    }
}

/// A shared handle to the events recorded by an installed capturing subscriber.
///
/// Cloning shares the same underlying buffer, so a clone kept by a test observes
/// the same events as the installed subscriber.
#[derive(Clone)]
pub struct CaptureHandle {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl CaptureHandle {
    /// A snapshot of the events recorded so far, in emission order.
    #[must_use]
    pub fn events(&self) -> Vec<CapturedEvent> {
        self.events.lock().expect("capture buffer poisoned").clone()
    }

    /// The number of events recorded so far.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.lock().expect("capture buffer poisoned").len()
    }

    /// Whether no events have been recorded yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Discards all recorded events, so subsequent assertions see only events
    /// emitted after this call.
    pub fn clear(&self) {
        self.events.lock().expect("capture buffer poisoned").clear();
    }

    /// A snapshot filtered to events whose `target` equals `target`.
    #[must_use]
    pub fn events_for_target(&self, target: &str) -> Vec<CapturedEvent> {
        self.events
            .lock()
            .expect("capture buffer poisoned")
            .iter()
            .filter(|e| e.target == target)
            .cloned()
            .collect()
    }
}

/// Installs a capturing subscriber as the thread-local `tracing` default.
///
/// Returns the [`CaptureHandle`] for inspecting recorded events and a
/// [`DefaultGuard`] that keeps the subscriber active until it is dropped; on
/// drop, the previous thread-local default is restored. Hold the guard for as
/// long as capture is wanted:
///
/// ```
/// use tracing::warn;
///
/// let (events, _guard) = komorebi::capture::install();
/// warn!(target: "canopy::lifecycle", event = "daemon_stop");
/// assert_eq!(events.len(), 1);
/// ```
#[must_use]
pub fn install() -> (CaptureHandle, DefaultGuard) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let layer = CaptureLayer {
        events: Arc::clone(&events),
    };
    let subscriber = Registry::default().with(layer);
    let guard = tracing::subscriber::set_default(subscriber);
    (CaptureHandle { events }, guard)
}

/// Runs `f` with a capturing subscriber installed on the current thread and
/// returns the events it emitted alongside `f`'s result.
///
/// This is the ergonomic form for the common case: install, run, snapshot, tear
/// down -- all scoped to the closure. See the [module docs](self) for an example.
pub fn with_capture<R>(f: impl FnOnce() -> R) -> (Vec<CapturedEvent>, R) {
    let (handle, guard) = install();
    let result = f();
    drop(guard);
    (handle.events(), result)
}

/// The `tracing_subscriber` layer that records each event into the shared buffer.
struct CaptureLayer {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        let captured = CapturedEvent {
            target: meta.target().to_string(),
            level: *meta.level(),
            name: meta.name().to_string(),
            fields: visitor.fields,
        };
        self.events
            .lock()
            .expect("capture buffer poisoned")
            .push(captured);
    }
}

/// Records each field's value as a string: raw text for string-valued fields,
/// the `Debug` rendering for everything else (numbers, bools, the message).
#[derive(Default)]
struct FieldVisitor {
    fields: BTreeMap<String, String>,
}

impl Visit for FieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.fields
            .insert(field.name().to_string(), format!("{value:?}"));
    }
}
