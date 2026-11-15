// Copyright (c) Michael Grier.

use super::*;

#[test]
fn sealed_waits_preserve_the_level_and_mix_with_ordinary_targets() {
    for _ in 0..12 {
        let first = EventFdSignal::new().unwrap();
        let second = EventFdSignal::new().unwrap();
        let sealed = first.sealed_wait_only().unwrap();
        let objects = [sealed.wait_object(), second.as_fd().into()];
        assert!(!sealed.wait(Duration::ZERO).unwrap());
        assert_eq!(wait_native_any(&objects, Duration::ZERO).unwrap(), None);
        second.raise().unwrap();
        assert_eq!(wait_native_any(&objects, Duration::ZERO).unwrap(), Some(1));
        assert!(second.wait(Duration::ZERO).unwrap());
        second.lower().unwrap();
        first.raise().unwrap();
        assert_eq!(wait_native_any(&objects, Duration::ZERO).unwrap(), Some(0));
        assert!(sealed.wait(Duration::ZERO).unwrap());
        first.lower().unwrap();
        assert!(!sealed.wait(Duration::ZERO).unwrap());
    }
}

#[test]
fn sealed_clones_keep_the_event_open_and_debug_does_not_lend_the_number() {
    fn shareable<T: Clone + Send + Sync>() {}
    shareable::<SealedWaitOnlyFd>();
    let signal = EventFdSignal::new().unwrap();
    signal.raise().unwrap();
    let sealed = signal.sealed_wait_only().unwrap();
    let clone = sealed.clone();
    assert_eq!(format!("{sealed:?}"), "SealedWaitOnlyFd { .. }");
    assert_eq!(
        format!("{:?}", sealed.wait_object()),
        "SealedWaitObject { .. }"
    );
    drop(signal);
    drop(sealed);
    assert!(clone.wait(Duration::ZERO).unwrap());
    assert_eq!(
        wait_native_any(&[(&clone).into()], Duration::ZERO).unwrap(),
        Some(0)
    );
}

#[test]
fn sealed_wait_timeout_is_bounded_and_an_empty_set_is_refused() {
    let signal = EventFdSignal::new().unwrap();
    let sealed = signal.sealed_wait_only().unwrap();
    let timeout = Duration::from_millis(2);
    let started = std::time::Instant::now();
    assert_eq!(
        wait_native_any(&[sealed.wait_object()], timeout).unwrap(),
        None
    );
    assert!(started.elapsed() >= timeout);
    assert!(wait_native_any(&[], Duration::ZERO).is_err());
}

#[test]
fn sealed_watches_use_the_existing_service_without_consuming_the_signal() {
    let signal = Arc::new(EventFdSignal::new().unwrap());
    let sealed = signal.sealed_wait_only().unwrap();
    let owner = Arc::clone(&signal);
    let (sender, receiver) = std::sync::mpsc::channel();
    let watch = sealed
        .watch(move || {
            owner.lower().unwrap();
            sender.send(()).unwrap();
        })
        .unwrap();
    signal.raise().unwrap();
    test_timers::mpsc::recv_timeout(&receiver, Duration::from_secs(3)).unwrap();
    assert!(!sealed.wait(Duration::ZERO).unwrap());
    drop(watch);
}

#[test]
fn mixed_waits_report_the_first_ready_index_without_consuming_any_target() {
    for count in 1..=12 {
        let signals: Vec<_> = (0..count).map(|_| EventFdSignal::new().unwrap()).collect();
        let waits: Vec<_> = signals
            .iter()
            .map(|signal| signal.sealed_wait_only().unwrap())
            .collect();
        let objects: Vec<_> = waits.iter().map(SealedWaitOnlyFd::wait_object).collect();
        signals[count - 1].raise().unwrap();
        assert_eq!(
            wait_native_any(&objects, Duration::ZERO).unwrap(),
            Some(count - 1)
        );
        signals[0].raise().unwrap();
        assert_eq!(wait_native_any(&objects, Duration::ZERO).unwrap(), Some(0));
        assert!(waits[count - 1].wait(Duration::ZERO).unwrap());
        assert!(waits[0].wait(Duration::ZERO).unwrap());
    }
}

#[test]
fn a_hung_up_ordinary_target_is_an_error_not_a_timeout() {
    let (reader, peer) = std::os::unix::net::UnixStream::pair().unwrap();
    let object = SealedWaitObject::from(reader.as_fd());
    drop(peer);
    assert!(wait_native_any(&[object], Duration::ZERO).is_err());
}
