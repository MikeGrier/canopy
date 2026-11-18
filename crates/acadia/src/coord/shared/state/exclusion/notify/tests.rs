// Copyright (c) Michael Grier.

use super::*;
use crate::coord::{ModificationEnd, Purpose, WriterExclusion};

fn state() -> SharedState {
    let fs = Arc::new(crate::MemFs::new());
    fs.create_dir_all(Path::new("/notify")).unwrap();
    let state = SharedState::open(fs, Path::new("/notify")).unwrap();
    state
        .acquire(Waiting::Wait)
        .unwrap()
        .complete_recovery()
        .unwrap();
    state
}

#[test]
fn opening_and_closing_count_changes_ring_once_for_normal_notifications() {
    let state = state();
    for _ in 0..12 {
        let before = state.sample().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let notice = state
            .notify(
                before,
                Instant::now() + Duration::from_secs(1),
                Box::new(move || {
                    let _ = sender.send(());
                }),
            )
            .unwrap();
        let mut hold = state.acquire(Waiting::Wait).unwrap();
        hold.begin_modification(Purpose::Ordinary).unwrap();
        receiver.recv().unwrap();
        assert!(!state.sample().unwrap().is_settled());
        assert!(receiver.try_recv().is_err());
        hold.end_modification(ModificationEnd::Settled).unwrap();
        drop(hold);
        drop(notice);
        assert_eq!(state.sample().unwrap().get(), before.get() + 2);
    }
}

#[test]
fn a_past_deadline_rings_without_a_count_change() {
    let state = state();
    let before = state.sample().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let notice = state
        .notify(
            before,
            Instant::now(),
            Box::new(move || {
                let _ = sender.send(());
            }),
        )
        .unwrap();
    receiver.recv().unwrap();
    assert_eq!(state.sample().unwrap(), before);
    drop(notice);
}

#[test]
fn cancellation_withdraws_the_callback_and_a_panicking_one_does_not_stop_others() {
    let state = state();
    let before = state.sample().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let cancelled = state
        .notify(
            before,
            Instant::now() + Duration::from_secs(10),
            Box::new(move || {
                let _ = sender.send(());
            }),
        )
        .unwrap();
    drop(cancelled);
    assert!(receiver.recv().is_err());
    let panic_notice = state
        .notify(
            before,
            Instant::now(),
            Box::new(|| panic!("notification stopped")),
        )
        .unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let good = state
        .notify(
            before,
            Instant::now(),
            Box::new(move || {
                let _ = sender.send(());
            }),
        )
        .unwrap();
    receiver.recv().unwrap();
    drop(panic_notice);
    drop(good);
}

#[test]
fn abandonment_causes_sampling_failure_and_rings_outstanding_notices() {
    let state = state();
    let (sender, receiver) = std::sync::mpsc::channel();
    let notice = state
        .notify(
            state.sample().unwrap(),
            Instant::now() + Duration::from_secs(1),
            Box::new(move || {
                let _ = sender.send(());
            }),
        )
        .unwrap();
    state.abandon();
    receiver.recv().unwrap();
    assert!(matches!(state.sample(), Err(CoordError::Abandoned)));
    drop(notice);
}

/// Registration makes no filesystem call while it holds the poller's mutex, so a filesystem
/// callback that drops a notice -- which takes that mutex -- cannot stop the poller.
#[test]
fn a_registration_whose_filesystem_drops_another_notice_does_not_stop_the_poller() {
    let other = state();
    let outstanding: Arc<Mutex<Option<Box<dyn Notice>>>> = Arc::new(Mutex::new(Some(
        other
            .notify(
                other.sample().unwrap(),
                Instant::now() + Duration::from_secs(60),
                Box::new(|| {}),
            )
            .unwrap(),
    )));
    let dropping = Arc::clone(&outstanding);
    let gate = crate::FaultGate::closed();
    let policy = crate::FaultPolicy::new().inject_gated(
        crate::FaultOp::BootIdentity,
        crate::PathMatcher::Predicate(Arc::new(|_| true)),
        crate::Fault::observe_with(move |_, _| {
            drop(dropping.lock().unwrap().take());
        }),
        // `notify` checks liveness, then registration does; the second is the one that matters.
        crate::When::OnCall(2),
        gate.clone(),
    );
    let fs = Arc::new(crate::MemFs::new());
    fs.create_dir_all(Path::new("/observed")).unwrap();
    let observed = SharedState::open(
        Arc::new(crate::FaultyFs::new(fs.another_process(), policy)),
        Path::new("/observed"),
    )
    .unwrap();
    let sampled = observed.sample().unwrap();
    gate.open();
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let notice = observed.notify(
            sampled,
            Instant::now() + Duration::from_secs(60),
            Box::new(|| {}),
        );
        let _ = done.send(notice.is_ok());
    });
    let registered =
        test_timers::mpsc::recv_timeout(&finished, Duration::from_secs(10)).expect("registered");
    assert!(registered);
    assert!(outstanding.lock().unwrap().is_none(), "the callback ran");
}
