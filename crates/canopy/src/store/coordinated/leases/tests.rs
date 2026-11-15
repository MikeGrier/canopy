// Copyright (c) Michael Grier.

use super::*;

#[test]
fn a_drain_is_ready_only_after_another_threads_nested_leases_end() {
    for _ in 0..12 {
        let leases = Arc::new(Leases::default());
        let (started, ready) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let worker = Arc::clone(&leases);
        let thread = std::thread::spawn(move || {
            let outer = worker.enter().unwrap();
            let inner = worker.enter().unwrap();
            started.send(()).unwrap();
            released.recv().unwrap();
            drop(outer);
            drop(inner);
        });
        ready.recv().unwrap();
        let drain = leases.begin().unwrap();
        assert!(!leases.ready(&drain).unwrap());
        assert!(matches!(leases.enter(), Err(LeaseError::Draining)));
        release.send(()).unwrap();
        leases.wait(&drain).unwrap();
        assert!(leases.cancel(&drain));
        drop(leases.enter().unwrap());
        thread.join().unwrap();
    }
}

#[test]
fn an_upgrade_from_the_callers_operation_is_refused_without_draining() {
    let leases = Arc::new(Leases::default());
    let lease = leases.enter().unwrap();
    assert!(matches!(leases.begin(), Err(LeaseError::InsideOperation)));
    drop(lease);
    let drain = leases.begin().unwrap();
    leases.wait(&drain).unwrap();
    assert!(leases.cancel(&drain));
}

#[test]
fn cancelling_a_drain_wakes_waiters_and_does_not_cancel_a_later_drain() {
    let leases = Arc::new(Leases::default());
    let first = leases.begin().unwrap();
    assert!(matches!(leases.begin(), Err(LeaseError::Draining)));
    assert!(leases.cancel(&first));
    assert_eq!(leases.wait(&first), Err(LeaseError::Cancelled));
    let second = leases.begin().unwrap();
    assert!(!leases.cancel(&first));
    assert!(leases.ready(&second).unwrap());
    assert!(leases.cancel(&second));
}

#[test]
fn detached_operations_are_refused_until_admission_restores_the_attachment() {
    let leases = Arc::new(Leases::default());
    let token = leases.begin().unwrap();
    leases.detach(&token).unwrap();
    assert!(matches!(leases.enter(), Err(LeaseError::Detached)));
    assert!(matches!(leases.begin(), Err(LeaseError::Detached)));
    leases.attached();
    drop(leases.enter().unwrap());
}
