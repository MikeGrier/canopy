// Copyright (c) Michael Grier.

use super::*;
use win_sync_sys::{Alertable, WaitAnyOutcome, wait_for_all, wait_for_any};
use windows_threadpool_sys::wait::{SealedThreadpoolWait, SealedWaitableHandle};

#[test]
fn sealed_event_waits_preserve_the_level_and_mix_with_ordinary_events() {
    for _ in 0..12 {
        let signal = EventSignal::new().unwrap();
        let sealed = signal.sealed_wait_only().unwrap();
        let other = Event::new(ResetMode::Manual, false).unwrap();
        let objects = [sealed.wait_object(), other.as_handle().into()];
        assert!(!wait_sealed(&sealed, Duration::ZERO).unwrap());
        assert_eq!(
            wait_for_any(&objects, Some(Duration::ZERO), Alertable::No).unwrap(),
            WaitAnyOutcome::TimedOut
        );
        other.set().unwrap();
        assert_eq!(
            wait_for_any(&objects, Some(Duration::ZERO), Alertable::No).unwrap(),
            WaitAnyOutcome::Signalled(1)
        );
        signal.raise().unwrap();
        assert_eq!(
            wait_for_all(&objects, Some(Duration::ZERO), Alertable::No).unwrap(),
            WaitOutcome::Signalled
        );
        assert!(wait_sealed(&sealed, Duration::ZERO).unwrap());
        other.reset().unwrap();
        signal.lower().unwrap();
        assert!(!wait_sealed(&sealed, Duration::ZERO).unwrap());
    }
}

#[test]
fn sealed_threadpool_callbacks_rearm_without_a_handle_accessor() {
    let signal = Arc::new(EventSignal::new().unwrap());
    let sealed = signal.sealed_wait_only().unwrap();
    let owner = Arc::clone(&signal);
    let (sender, receiver) = std::sync::mpsc::channel();
    let wait = SealedThreadpoolWait::new(
        SealedWaitableHandle::from(sealed),
        move |activation| {
            owner.lower().unwrap();
            activation.rearm(None);
            let _ = sender.send(());
        },
        None,
    )
    .unwrap();
    wait.arm(None);
    for _ in 0..12 {
        signal.raise().unwrap();
        test_timers::mpsc::recv_timeout(&receiver, Duration::from_secs(3)).unwrap();
    }
    wait.stop_and_drain();
    assert!(!signal.wait(Duration::ZERO).unwrap());
}

#[test]
fn sealed_event_clones_keep_the_object_alive_and_hide_the_handle() {
    fn shareable<T: Clone + Send + Sync>() {}
    shareable::<SealedWaitOnlyEvent>();
    let signal = EventSignal::new().unwrap();
    signal.raise().unwrap();
    let sealed = signal.sealed_wait_only().unwrap();
    let clone = sealed.clone();
    drop(signal);
    drop(sealed);
    assert!(wait_sealed(&clone, Duration::ZERO).unwrap());
}
