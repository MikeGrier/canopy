// Copyright (c) Michael Grier.

//! Unit tests  subscriber install is idempotent/safe,
//! `LogConfig` filtering selects the expected directives and events, and the
//! `capture` helper records emitted events with their fields.
//!
//! Capture-based tests use the thread-local capturing subscriber, so they are
//! deterministic and do not race with each other or with the global-default
//! install tests below.

use std::sync::{Arc, Mutex};

use tracing::{Level, debug, error, info, trace, warn};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::Registry;

use crate::LogConfig;
use crate::capture::{install, with_capture};
use crate::conventions::{fields, outcomes, phases, results, targets};

// -- capture helper (LOG-3) ------------------------------------------------

#[test]
fn with_capture_records_single_event_target_and_message() {
    let (events, ()) = with_capture(|| {
        info!(target: targets::LIFECYCLE, "daemon started on {}", 42);
    });

    assert_eq!(events.len(), 1);
    let e = &events[0];
    assert_eq!(e.target(), targets::LIFECYCLE);
    assert_eq!(e.level(), Level::INFO);
    assert_eq!(e.message(), Some("daemon started on 42"));
}

#[test]
fn with_capture_returns_the_closure_result() {
    let (events, result) = with_capture(|| {
        info!(target: targets::LIFECYCLE, event = "x");
        99_u32
    });

    assert_eq!(result, 99);
    assert_eq!(events.len(), 1);
}

#[test]
fn capture_records_convention_fields() {
    let (events, ()) = with_capture(|| {
        info!(
            target: targets::RECOVERY,
            event = "replay_log",
            phase = phases::REPLAY_LOG,
            outcome = outcomes::COMPLETED,
        );
    });

    let e = &events[0];
    assert_eq!(e.target(), targets::RECOVERY);
    assert_eq!(e.field(fields::EVENT), Some("replay_log"));
    assert_eq!(e.field(fields::PHASE), Some(phases::REPLAY_LOG));
    assert_eq!(e.field(fields::OUTCOME), Some(outcomes::COMPLETED));
}

#[test]
fn capture_keeps_string_field_raw_without_quotes() {
    let (events, ()) = with_capture(|| {
        info!(target: targets::LIFECYCLE, path = "/data/shire");
    });

    assert_eq!(events[0].field(fields::PATH), Some("/data/shire"));
}

#[test]
fn capture_renders_numeric_and_bool_fields_via_debug() {
    let (events, ()) = with_capture(|| {
        info!(target: targets::VERIFY_REPAIR, generation = 7_u64, quorum = 2_u64, agreed = true);
    });

    let e = &events[0];
    assert_eq!(e.field(fields::GENERATION), Some("7"));
    assert_eq!(e.field(fields::QUORUM), Some("2"));
    assert_eq!(e.field("agreed"), Some("true"));
}

#[test]
fn capture_preserves_emission_order_and_levels() {
    let (events, ()) = with_capture(|| {
        info!(target: targets::LIFECYCLE, event = "a");
        info!(target: targets::LIFECYCLE, event = "b");
        warn!(target: targets::RECOVERY, event = "c");
    });

    assert_eq!(events.len(), 3);
    assert_eq!(events[0].field(fields::EVENT), Some("a"));
    assert_eq!(events[1].field(fields::EVENT), Some("b"));
    assert_eq!(events[2].field(fields::EVENT), Some("c"));
    assert_eq!(events[2].level(), Level::WARN);
}

#[test]
fn capture_records_error_level_and_result_field() {
    let (events, ()) = with_capture(|| {
        error!(target: targets::VERIFY_REPAIR, result = results::UNREPAIRABLE);
    });

    assert_eq!(events[0].level(), Level::ERROR);
    assert_eq!(events[0].field(fields::RESULT), Some(results::UNREPAIRABLE));
}

#[test]
fn stream_vocabulary_constants_have_expected_spellings() {
    use crate::conventions::stream_reasons;

    // The canonical spellings a capturing subscriber (and thus a consumer's
    // test) matches on; changing any is a breaking observability change.
    assert_eq!(targets::STREAM, "canopy::stream");
    assert_eq!(fields::FORWARDED, "forwarded");
    assert_eq!(stream_reasons::COMPLETED, "completed");
    assert_eq!(stream_reasons::RELAY_DROPPED, "relay_dropped");
    assert_eq!(stream_reasons::READ_ERROR, "read_error");
    assert_eq!(stream_reasons::STRANDED, "stranded");
}

#[test]
fn capture_records_a_pump_end_stream_event() {
    use crate::conventions::stream_reasons;

    let (events, ()) = with_capture(|| {
        info!(
            target: targets::STREAM,
            event = "pump_end",
            ticket = "TCKT",
            channel = "stdout",
            reason = stream_reasons::COMPLETED,
            forwarded = 4096_u64,
            length = 4096_u64,
        );
    });

    let e = &events[0];
    assert_eq!(e.target(), targets::STREAM);
    assert_eq!(e.field(fields::EVENT), Some("pump_end"));
    assert_eq!(e.field(fields::TICKET), Some("TCKT"));
    assert_eq!(e.field(fields::CHANNEL), Some("stdout"));
    assert_eq!(e.field(fields::REASON), Some(stream_reasons::COMPLETED));
    assert_eq!(e.field(fields::FORWARDED), Some("4096"));
}

#[test]
fn capture_events_for_target_filters_by_target() {
    let (handle, guard) = install();
    info!(target: targets::LIFECYCLE, event = "x");
    info!(target: targets::RECOVERY, event = "y");
    info!(target: targets::LIFECYCLE, event = "z");
    drop(guard);

    assert_eq!(handle.events().len(), 3);
    let life = handle.events_for_target(targets::LIFECYCLE);
    assert_eq!(life.len(), 2);
    assert!(life.iter().all(|e| e.target() == targets::LIFECYCLE));
}

#[test]
fn capture_handle_clear_discards_prior_events() {
    let (handle, guard) = install();
    info!(target: targets::LIFECYCLE, event = "x");
    assert_eq!(handle.len(), 1);

    handle.clear();
    assert!(handle.is_empty());

    info!(target: targets::LIFECYCLE, event = "y");
    assert_eq!(handle.len(), 1);
    drop(guard);
}

#[test]
fn capture_guard_scopes_to_its_lifetime() {
    let (handle, guard) = install();
    info!(target: targets::LIFECYCLE, event = "inside");
    drop(guard);

    // After the guard drops the capturing subscriber is no longer this thread's
    // default; a later event is not recorded by the handle. Emit at trace so any
    // global default installed by another test drops it rather than printing.
    trace!(target: targets::LIFECYCLE, event = "outside");

    assert_eq!(handle.len(), 1);
    assert_eq!(handle.events()[0].field(fields::EVENT), Some("inside"));
}

#[test]
fn capture_reports_absent_fields() {
    let (events, ()) = with_capture(|| {
        info!(target: targets::LIFECYCLE, event = "x");
    });

    let e = &events[0];
    assert!(e.has_field(fields::EVENT));
    assert_eq!(e.field(fields::OUTCOME), None);
    assert!(!e.has_field(fields::OUTCOME));
}

#[test]
fn capture_with_no_events_is_empty() {
    let (events, value) = with_capture(|| 5_i32);
    assert!(events.is_empty());
    assert_eq!(value, 5);

    let (handle, guard) = install();
    assert!(handle.is_empty());
    drop(guard);
}

// -- filtering (LOG-1) -----------------------------------------------------

#[test]
fn filter_uses_default_directives_when_env_var_unset() {
    let cfg = LogConfig::default()
        .with_env_var("KOMOREBI_TEST_FILTER_UNSET_XYZ")
        .with_default_directives("warn,canopy::recovery=debug");

    let shown = cfg.env_filter().to_string();
    assert!(shown.contains("warn"), "{shown}");
    assert!(shown.contains("canopy::recovery=debug"), "{shown}");
}

#[test]
fn filter_uses_default_when_env_var_name_is_empty() {
    let cfg = LogConfig::default()
        .with_env_var("")
        .with_default_directives("error,canopy::lifecycle=info");

    let shown = cfg.env_filter().to_string();
    assert!(shown.contains("error"), "{shown}");
    assert!(shown.contains("canopy::lifecycle=info"), "{shown}");
}

#[test]
fn filter_env_var_overrides_default_directives() {
    let name = "KOMOREBI_TEST_FILTER_OVERRIDE_UNIQUE";
    // SAFETY: the variable name is unique to this test, so no other thread
    // reads or writes it while it is briefly set.
    unsafe {
        std::env::set_var(name, "trace,canopy::verify_repair=warn");
    }
    let cfg = LogConfig::default()
        .with_env_var(name)
        .with_default_directives("info");
    let shown = cfg.env_filter().to_string();
    // SAFETY: as above.
    unsafe {
        std::env::remove_var(name);
    }

    assert!(shown.contains("trace"), "{shown}");
    assert!(shown.contains("canopy::verify_repair=warn"), "{shown}");
}

#[test]
fn filter_drops_unparseable_directive_but_keeps_valid_ones() {
    let cfg = LogConfig::default()
        .with_env_var("KOMOREBI_TEST_FILTER_BAD_UNIQUE")
        .with_default_directives("info,this is not valid===,canopy::recovery=debug");

    // parse_lossy must not panic; the valid directives survive.
    let shown = cfg.env_filter().to_string();
    assert!(shown.contains("info"), "{shown}");
    assert!(shown.contains("canopy::recovery=debug"), "{shown}");
}

/// A minimal `tracing_subscriber` layer that records each event's target and
/// level -- used to prove that a `LogConfig` filter admits exactly the expected
/// events end to end.
#[derive(Clone, Default)]
struct TargetRecorder {
    seen: Arc<Mutex<Vec<(String, Level)>>>,
}

impl<S: tracing::Subscriber> Layer<S> for TargetRecorder {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        self.seen
            .lock()
            .expect("recorder poisoned")
            .push((meta.target().to_string(), *meta.level()));
    }
}

#[test]
fn filter_selects_events_by_target_and_level() {
    let cfg = LogConfig::default()
        .with_env_var("KOMOREBI_TEST_FILTER_E2E_UNIQUE")
        .with_default_directives("info,canopy::recovery=debug");

    let seen: Arc<Mutex<Vec<(String, Level)>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = TargetRecorder {
        seen: Arc::clone(&seen),
    };
    let subscriber = Registry::default().with(cfg.env_filter()).with(recorder);

    {
        let _guard = tracing::subscriber::set_default(subscriber);
        info!(target: targets::LIFECYCLE, "admitted: info >= info");
        debug!(target: targets::LIFECYCLE, "dropped: debug < info");
        debug!(target: targets::RECOVERY, "admitted: recovery=debug");
        trace!(target: targets::RECOVERY, "dropped: trace < debug");
        info!(target: targets::VERIFY_REPAIR, "admitted: info >= info");
        debug!(target: targets::VERIFY_REPAIR, "dropped: debug < info");
    }

    let got = seen.lock().expect("recorder poisoned").clone();
    assert_eq!(got.len(), 3, "{got:?}");

    let seen_targets: Vec<&str> = got.iter().map(|(t, _)| t.as_str()).collect();
    assert!(seen_targets.contains(&targets::LIFECYCLE));
    assert!(seen_targets.contains(&targets::RECOVERY));
    assert!(seen_targets.contains(&targets::VERIFY_REPAIR));

    let recovery_levels: Vec<Level> = got
        .iter()
        .filter(|(t, _)| t == targets::RECOVERY)
        .map(|(_, l)| *l)
        .collect();
    assert_eq!(recovery_levels, vec![Level::DEBUG]);
}

// -- subscriber install (LOG-1) --------------------------------------------

#[test]
fn install_is_idempotent_safe_and_reports_init_error() {
    // The first global install in the process may win here or may already have
    // been performed by another test; either way it must not panic.
    let _ = LogConfig::default().install();

    // Once a global default exists, a further install fails rather than
    // panicking or displacing the live subscriber.
    let second = LogConfig::default().install();
    assert!(second.is_err());

    let err = second.unwrap_err();
    assert!(!err.message().is_empty());
    assert!(
        err.to_string()
            .starts_with("could not install tracing subscriber:")
    );

    // The init() convenience shares the same global default and is likewise safe.
    assert!(crate::init().is_err());
}
