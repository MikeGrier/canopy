// Copyright (c) Michael Grier.

//! Attached writers take turns under the coordination contract's exclusion.
//!
//! These run over [`MemFs`], whose exclusion scope is one process. That is the faithful
//! scope for that filesystem, but it means **nothing here proves cross-process behaviour**
//! -- that is [tests/root_lock.rs](../../../canopy-lock-holder/tests/root_lock.rs), which spawns a real second
//! process.

use super::*;
use crate::store::LOCK_NAME;
use acadia::{Fault, FaultOp, FaultPolicy, FaultyFs, VfsErrorKind, When};

#[test]
fn a_second_writer_opens_beside_an_attached_writer() {
    let (vfs, store) = fresh_store();

    let second = Store::open_unsourced(Arc::clone(&vfs), store.root().to_path_buf()).unwrap();
    assert_eq!(second.store_id(), store.store_id());
}

#[test]
fn opening_does_not_take_the_legacy_root_lock() {
    let gate = acadia::FaultGate::closed();
    let root = PathBuf::from("store");
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::LockExclusive,
        acadia::PathMatcher::exact(root.join(LOCK_NAME)),
        Fault::io_error(),
        When::Always,
        gate.clone(),
    );
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(MemFs::new(), policy));
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init_unsourced(vfs.clone(), root.clone(), test_config()).unwrap();
    drop(store);
    gate.open();

    Store::open_unsourced(vfs, root).unwrap();
}

#[test]
fn creation_uses_coordination_rather_than_the_legacy_root_lock() {
    let vfs: Arc<dyn Vfs> = Arc::new(MemFs::new());
    let root = PathBuf::from("/held");
    vfs.create_dir_all(&root).unwrap();
    let _held = vfs.lock_exclusive(&root.join(LOCK_NAME)).unwrap();

    Store::init_unsourced(Arc::clone(&vfs), root, test_config()).unwrap();
}

/// A root that is plainly occupied is refused before the lock is taken, so `init` leaves
/// no lock file behind in a directory that was never a store.
#[test]
fn init_over_an_occupied_root_leaves_no_lock_file() {
    let vfs: Arc<dyn Vfs> = Arc::new(MemFs::new());
    let root = PathBuf::from("/occupied");
    vfs.create_dir_all(&root).unwrap();
    vfs.write(&root.join("data.txt"), b"not a store").unwrap();

    let refused = Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config())
        .expect_err("an occupied root is refused");

    assert!(
        matches!(refused, StoreError::NotEmpty { .. }),
        "got {refused:?}"
    );
    assert_eq!(
        vfs.metadata(&root.join(LOCK_NAME)).unwrap_err().kind(),
        VfsErrorKind::NotFound,
        "the refusal must not leave a lock file"
    );
}

/// A completed creation's marker refuses a second creation.
#[test]
fn init_after_a_released_init_is_refused_as_already_initialized() {
    let (vfs, store) = fresh_store();
    let root = store.root().to_path_buf();
    drop(store);

    let refused = Store::init_unsourced(Arc::clone(&vfs), root, test_config())
        .expect_err("an initialized root refuses a second init");
    assert!(
        matches!(refused, StoreError::AlreadyInitialized { .. }),
        "got {refused:?}"
    );
}

/// Releasing is what makes the lock usable rather than a one-shot trap.
#[test]
fn dropping_the_store_releases_the_root() {
    let (vfs, store) = fresh_store();
    let root = store.root().to_path_buf();

    drop(store);

    let reopened =
        Store::open_unsourced(Arc::clone(&vfs), root).expect("the root is free once released");
    drop(reopened);
}

#[test]
fn a_clone_keeps_the_attachment_without_excluding_another_writer() {
    let (vfs, store) = fresh_store();
    let root = store.root().to_path_buf();
    let clone = store.clone();

    drop(store);
    let second = Store::open_unsourced(Arc::clone(&vfs), root.clone()).unwrap();
    assert_eq!(second.store_id(), clone.store_id());

    drop(clone);
    drop(Store::open_unsourced(Arc::clone(&vfs), root).expect("the last handle released it"));
}

/// A clone does not re-acquire. If it did, the store would deadlock against itself the
/// moment a handle was cloned for another thread.
#[test]
fn cloning_a_store_does_not_re_acquire() {
    let (_vfs, store) = fresh_store();

    let clones: Vec<Store> = (0..4).map(|_| store.clone()).collect();

    // Each clone is a working handle, not a husk: reading through one proves the clone
    // carries the store rather than merely compiling.
    for clone in &clones {
        assert!(clone.commit_log().read().expect("read log").is_empty());
    }
}

/// Two roots are two locks. Holding one says nothing about the other, which is what lets
/// one process hold several stores side by side.
#[test]
fn distinct_roots_are_independently_locked() {
    let (vfs, first) = fresh_store();
    let second_root = PathBuf::from("another-store");
    vfs.create_dir_all(&second_root).expect("create root");

    let second = Store::init_unsourced(Arc::clone(&vfs), second_root, test_config())
        .expect("a different root is not excluded by the first's lock");

    assert_ne!(first.root(), second.root());
}

/// A failed `open` releases whatever it took on the way in. Otherwise the first bad open
/// of a root would poison it for the rest of the process's life.
#[test]
fn a_failed_open_leaves_the_root_free() {
    let (vfs, root) = fresh_root();

    // No marker yet, so this fails validation -- before the lock is even taken.
    let refused = Store::open_unsourced(Arc::clone(&vfs), root.clone()).expect_err("not a store");
    assert!(matches!(refused, StoreError::NotAStore { .. }));

    drop(
        Store::init_unsourced(Arc::clone(&vfs), root, test_config())
            .expect("the root was not poisoned"),
    );
}

/// `open` must not litter: a directory that turns out not to be a store gets no lock file.
/// This is why validation runs before acquisition.
#[test]
fn a_refused_open_drops_no_lock_file_into_a_non_store() {
    let (vfs, root) = fresh_root();

    Store::open_unsourced(Arc::clone(&vfs), root.clone()).expect_err("not a store");

    let entries = vfs.read_dir(&root).expect("read root");
    assert!(
        !entries.iter().any(|e| e.file_name == LOCK_NAME),
        "a non-store must not be given a lock file, found {entries:?}"
    );
}

#[test]
fn a_legacy_lock_file_does_not_exclude_current_writers() {
    let (vfs, store) = fresh_store();
    let root = store.root().to_path_buf();
    let legacy = vfs.lock_exclusive(&root.join(LOCK_NAME)).unwrap();
    drop(store);

    assert!(
        vfs.exists(&root.join(LOCK_NAME)).expect("stat lock file"),
        "the file is left behind on release"
    );

    drop(
        Store::open_unsourced(Arc::clone(&vfs), root).expect("a leftover lock file wedges nothing"),
    );
    drop(legacy);
}

/// Clones of one store are one writer: two of them racing a create-if-absent of the same
/// document on different threads must not both succeed.
#[test]
fn clones_racing_a_conditional_create_commit_exactly_once() {
    let (_vfs, store) = fresh_store();
    for round in 0..20 {
        let document = doc(&format!("race{round}"));
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let racers: Vec<_> = (0..2)
            .map(|racer| {
                let store = store.clone();
                let document = document.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    store
                        .update_document_as(
                            Swap::conditional_absent(
                                name("contended"),
                                document,
                                body(format!("racer {racer}").as_bytes()),
                            ),
                            &nonce(&format!("R{round}X{racer}")),
                        )
                        .expect("no store error")
                })
            })
            .collect();
        let committed = racers
            .into_iter()
            .map(|racer| racer.join().expect("racer thread"))
            .filter(TransactionOutcome::is_committed)
            .count();
        assert_eq!(
            committed, 1,
            "round {round}: both racers created the document"
        );
    }
}

/// The writer gate is re-entrant on its holder's thread and exclusive across threads.
#[test]
fn the_writer_gate_nests_on_one_thread_and_excludes_others() {
    let (_vfs, store) = fresh_store();
    let outer = store.exclusive();
    let inner = store.exclusive();

    let (arriving_tx, arriving_rx) = std::sync::mpsc::channel();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let other = store.clone();
    let waiter = thread::spawn(move || {
        arriving_tx.send(()).expect("report arrival");
        let _held = other.exclusive();
        entered_tx.send(()).expect("report entry");
    });

    drop(inner);
    // Once the waiter is known to be at the gate, give it a real window in which to get
    // through wrongly. A bare `try_recv` would pass even if the waiter had not started yet.
    arriving_rx.recv().expect("the waiter reaches the gate");
    assert!(
        test_timers::mpsc::recv_timeout(&entered_rx, std::time::Duration::from_millis(50)).is_err(),
        "another thread entered while the gate was still held once"
    );
    drop(outer);
    entered_rx
        .recv()
        .expect("the waiter enters once the gate is free");
    waiter.join().expect("waiter thread");
}

/// A thread that panics while holding the gate releases it as it unwinds, so one failed
/// writer cannot wedge every other handle to the store.
#[test]
fn a_writer_that_panics_releases_the_gate() {
    let (_vfs, store) = fresh_store();
    let holder = store.clone();
    let panicked = thread::spawn(move || {
        let _held = holder.exclusive();
        panic!("a writer failing while it holds the gate");
    })
    .join();
    assert!(panicked.is_err());

    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let other = store.clone();
    let waiter = thread::spawn(move || {
        let _held = other.exclusive();
        entered_tx.send(()).expect("report entry");
    });
    test_timers::mpsc::recv_timeout(&entered_rx, std::time::Duration::from_secs(10))
        .expect("the gate was released by the unwinding writer");
    waiter.join().expect("waiter thread");
}

/// Every writer queued behind a holder is admitted once the holder releases; a ticket that
/// was skipped would leave its writer, and every one behind it, waiting forever.
#[test]
fn queued_writers_are_each_admitted_once_the_gate_is_released() {
    let (_vfs, store) = fresh_store();
    let outer = store.exclusive();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let waiters: Vec<_> = (0..3)
        .map(|id| {
            let handle = store.clone();
            let entered = entered_tx.clone();
            thread::spawn(move || {
                let _held = handle.exclusive();
                entered.send(id).expect("report entry");
            })
        })
        .collect();
    drop(entered_tx);
    drop(outer);

    let mut admitted: Vec<i32> = (0..3)
        .map(|_| {
            test_timers::mpsc::recv_timeout(&entered_rx, std::time::Duration::from_secs(10))
                .expect("every queued writer is admitted")
        })
        .collect();
    admitted.sort_unstable();
    assert_eq!(admitted, [0, 1, 2]);
    for waiter in waiters {
        waiter.join().expect("waiter thread");
    }
}

#[test]
fn independent_handles_alternate_logged_and_unlogged_writes() {
    let (vfs, first) = fresh_store();
    let second = Store::open_unsourced(Arc::clone(&vfs), first.root().to_path_buf()).unwrap();
    for index in 0..12 {
        let (writer, reader) = if index % 2 == 0 {
            (&first, &second)
        } else {
            (&second, &first)
        };
        let document = doc(&format!("document{index}"));
        let content = body(format!("value{index}").as_bytes());
        let token = nonce(&format!("turn{index}"));
        if index % 3 == 0 {
            assert!(
                writer
                    .update_document_as(
                        Swap::new(name("turns"), document.clone(), content.clone()),
                        &token
                    )
                    .unwrap()
                    .is_committed()
            );
        } else {
            writer
                .write_document_replicated_as(name("turns"), &document, &content, &token)
                .unwrap();
        }
        assert_eq!(
            reader.read_body(name("turns"), &document).unwrap(),
            Some(content)
        );
    }
}

#[test]
fn identical_concurrent_creators_never_replace_a_completed_store() {
    for round in 0..12 {
        let (vfs, root) = fresh_root();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let creators: Vec<_> = (0..2)
            .map(|_| {
                let (vfs, root, barrier) = (Arc::clone(&vfs), root.clone(), Arc::clone(&barrier));
                thread::spawn(move || {
                    barrier.wait();
                    Store::init_unsourced(vfs, root, test_config())
                })
            })
            .collect();
        let outcomes: Vec<_> = creators
            .into_iter()
            .map(|creator| creator.join().unwrap())
            .collect();
        assert_eq!(
            outcomes.iter().filter(|outcome| outcome.is_ok()).count(),
            1,
            "round {round}"
        );
        assert!(
            outcomes
                .iter()
                .any(|outcome| matches!(outcome, Err(StoreError::AlreadyInitialized { .. }))),
            "round {round}: the losing creator reported {:?}",
            outcomes
                .iter()
                .filter_map(|o| o.as_ref().err())
                .collect::<Vec<_>>()
        );
        let store = outcomes.into_iter().find_map(Result::ok).unwrap();
        store
            .update_document_as(
                Swap::new(name("created"), doc("one"), body(b"original")),
                &nonce("created"),
            )
            .unwrap();
        assert!(matches!(
            Store::init_unsourced(vfs, root, test_config()),
            Err(StoreError::AlreadyInitialized { .. })
        ));
        assert_eq!(
            store.read_body(name("created"), &doc("one")).unwrap(),
            Some(body(b"original"))
        );
    }
}

#[test]
fn nested_guards_share_one_generation_even_when_dropped_out_of_order() {
    let (vfs, store) = fresh_store();
    let outer = store.exclusive();
    let generation = store.acquired_exclusion().unwrap().advanced_to();
    let inner = store.exclusive();
    assert_eq!(
        store.acquired_exclusion().unwrap().advanced_to(),
        generation
    );
    drop(outer);
    assert_eq!(
        store.acquired_exclusion().unwrap().advanced_to(),
        generation
    );
    drop(inner);
    assert!(store.acquired_exclusion().is_none());
    let other = Store::open_unsourced(vfs, store.root().to_path_buf()).unwrap();
    let _hold = other.exclusive();
    assert_ne!(
        other.acquired_exclusion().unwrap().advanced_to(),
        generation
    );
}
