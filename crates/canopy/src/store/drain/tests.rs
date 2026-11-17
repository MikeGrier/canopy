// Copyright (c) Michael Grier.

use super::*;
use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, MemFs, PathMatcher, When};
use std::sync::mpsc;
use std::time::Duration;

fn fixture() -> (Arc<dyn Vfs>, Store) {
    fixture_with(FaultPolicy::new())
}

fn fixture_with(policy: FaultPolicy) -> (Arc<dyn Vfs>, Store) {
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(MemFs::new(), policy));
    let root = PathBuf::from("drain-store");
    vfs.create_dir_all(&root).unwrap();
    let config = GenesisConfig::new(StoreId::new("drain-store").unwrap(), 0);
    let mut next = 0;
    let options = StoreOptions::with_nonce_source(
        Agent::new("drain-tests", "test", "drain fixture", 1),
        move || {
            next += 1;
            Nonce::new(&format!("n{next}")).unwrap()
        },
    );
    let store = Store::init(Arc::clone(&vfs), root, config, options).unwrap();
    (vfs, store)
}

fn document() -> DocumentName {
    DocumentName::new("one").unwrap()
}

fn node() -> NodePath {
    NodePath::parse("docs").unwrap()
}

fn seed(store: &Store) {
    store
        .write_document_replicated(node(), &document(), &DocumentBody::new(b"seed".to_vec()))
        .unwrap()
        .agent_stamp
        .unwrap();
}

struct IgnoreCommits;

impl crate::CommitObserver for IgnoreCommits {
    fn committed(&self, _node: &NodePath, _document: &DocumentName, _body: &DocumentBody) {}
}

struct RefuseDrainFromCommit(Store);

impl crate::CommitObserver for RefuseDrainFromCommit {
    fn committed(&self, _node: &NodePath, _document: &DocumentName, _body: &DocumentBody) {
        assert!(matches!(
            self.0.drain_upgrade(),
            Err(StoreError::UpgradeFromOperation { .. })
        ));
    }
}

#[test]
fn a_drained_writer_refuses_writes_and_cancellation_restores_it() {
    let (_, store) = fixture();
    let drain = store.drain_upgrade().unwrap();
    assert!(drain.is_ready().unwrap());
    assert!(drain.registration_error().is_none());
    assert!(matches!(
        store.writable(),
        Err(StoreError::AttachmentDraining { .. })
    ));
    drain.cancel().unwrap();
    assert!(matches!(
        drain.wait(),
        Err(StoreError::UpgradeCancelled { .. })
    ));
    drop(store.writable().unwrap());
}

#[test]
fn a_drain_refuses_every_work_surface_without_running_callbacks() {
    let (_, store) = fixture();
    seed(&store);
    let pointer = store.read_pointer(&node(), &document()).unwrap();
    let reference = pointer.pointer().target();
    let body = DocumentBody::new(b"seed".to_vec());
    let swaps = SwapSet::single(Swap::new(node(), document(), body.clone()));
    let drain = store.drain_upgrade().unwrap();
    let operations = [
        (
            "document",
            store.read_document(node(), &document()).map(|_| ()),
        ),
        (
            "snapshot",
            store.snapshot(|_| panic!("a refused snapshot must not call its reader")),
        ),
        ("children", store.list_children(node()).map(|_| ())),
        ("documents", store.list_documents().map(|_| ())),
        ("blob", store.read_blob(&reference).map(|_| ())),
        ("verify", store.verify_blob(&reference).map(|_| ())),
        (
            "pointer",
            store.read_pointer(&node(), &document()).map(|_| ()),
        ),
        ("properties", store.properties().map(|_| ())),
        ("handles", store.attached_handles().map(|_| ())),
        ("count", store.modifications().map(|_| ())),
        ("discovery", store.recovery_need().map(|_| ())),
        ("recovery", store.recovery().map(|_| ())),
        ("recover", store.recover().map(|_| ())),
        ("resolve", store.resolve().map(|_| ())),
        ("scrub", store.scrub_content().map(|_| ())),
        ("backlog", store.repair_reported_damage().map(|_| ())),
        ("quiesce", store.quiesce().map(|_| ())),
        ("log", store.commit_log_is_empty().map(|_| ())),
        ("status", store.recovery_required().map(|_| ())),
        (
            "description",
            store.set_agent_description("not changed").map(|_| ()),
        ),
        (
            "update",
            store
                .update_document(Swap::new(node(), document(), body.clone()))
                .map(|_| ()),
        ),
        (
            "pointer repair",
            store
                .repair_pointer(&node(), &document(), &pointer)
                .map(|_| ()),
        ),
        (
            "blob repair",
            store
                .repair_blob(&reference, body.as_bytes(), &[0], &[])
                .map(|_| ()),
        ),
        ("transaction", store.apply_transaction(&swaps).map(|_| ())),
        (
            "unlogged",
            store
                .write_document_replicated(node(), &document(), &body)
                .map(|_| ()),
        ),
        (
            "transform",
            store
                .transform(|_| -> Result<(), StoreError> {
                    panic!("a refused transform must not run")
                })
                .map(|_| ()),
        ),
        ("observer", store.observe(Arc::new(IgnoreCommits))),
        (
            "read once",
            crate::read_once(&store, &node(), &document()).map(|_| ()),
        ),
        (
            "read within",
            crate::read_within(&store, &node(), &document(), 2).map(|_| ()),
        ),
    ];
    for (label, outcome) in operations {
        assert!(
            matches!(outcome, Err(StoreError::AttachmentDraining { .. })),
            "{label}: {outcome:?}"
        );
    }
    drain.cancel().unwrap();
    assert!(matches!(
        store.read_document(node(), &document()).unwrap(),
        crate::Read::Found(_)
    ));
}

#[test]
fn drain_registry_changes_preserve_tiers_identity_and_store_data() {
    for _ in 0..12 {
        let (vfs, store) = fixture();
        let diagnostic = DiagnosticView::open(Arc::clone(&vfs), store.root.clone()).unwrap();
        let before = diagnostic.attached_handles().unwrap().live.pop().unwrap();
        let marker = vfs.read(&store.root.join(MARKER_NAME)).unwrap();
        let drain = store.clone().drain_upgrade().unwrap();
        let during = diagnostic.attached_handles().unwrap().live.pop().unwrap();
        assert!(during.draining);
        assert_eq!(during.token, before.token);
        assert_eq!(during.tiers, before.tiers);
        assert_eq!(during.agent_identifier, before.agent_identifier);
        assert!(drain.is_ready().unwrap());
        drain.wait().unwrap();
        drop(drain);
        assert_eq!(diagnostic.attached_handles().unwrap().live, [before]);
        assert_eq!(vfs.read(&store.root.join(MARKER_NAME)).unwrap(), marker);
    }
}

#[test]
fn an_operation_on_another_clone_finishes_before_the_drain_is_ready() {
    let (vfs, store) = fixture();
    seed(&store);
    let diagnostic = DiagnosticView::open(vfs, store.root.clone()).unwrap();
    let (started, entered) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let worker = store.clone();
    let operation = std::thread::spawn(move || {
        worker
            .snapshot(|view| {
                started.send(()).unwrap();
                released.recv().unwrap();
                view.body(node(), &document()).unwrap()
            })
            .unwrap()
    });
    entered.recv().unwrap();
    let drain = Arc::new(store.drain_upgrade().unwrap());
    assert!(!drain.is_ready().unwrap());
    assert!(diagnostic.attached_handles().unwrap().live[0].draining);
    let (completed, completion) = mpsc::channel();
    let waiting = Arc::clone(&drain);
    let waiter = std::thread::spawn(move || completed.send(waiting.wait()).unwrap());
    assert!(test_timers::mpsc::recv_timeout(&completion, Duration::from_millis(20)).is_err());
    assert!(matches!(
        store.clone().list_documents(),
        Err(StoreError::AttachmentDraining { .. })
    ));
    release.send(()).unwrap();
    assert!(operation.join().unwrap().is_some());
    test_timers::mpsc::recv_timeout(&completion, Duration::from_secs(5))
        .unwrap()
        .unwrap();
    waiter.join().unwrap();
    assert!(drain.is_ready().unwrap());
    drain.cancel().unwrap();
}

#[test]
fn cancelling_while_waiting_leaves_operations_and_attachment_usable() {
    let (_, store) = fixture();
    let (started, entered) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let worker = store.clone();
    let operation = std::thread::spawn(move || {
        worker
            .snapshot(|_| {
                started.send(()).unwrap();
                released.recv().unwrap();
            })
            .unwrap()
    });
    entered.recv().unwrap();
    let drain = Arc::new(store.drain_upgrade().unwrap());
    let waiting = Arc::clone(&drain);
    let (completed, completion) = mpsc::channel();
    let waiter = std::thread::spawn(move || completed.send(waiting.wait()).unwrap());
    drain.cancel().unwrap();
    assert!(matches!(
        test_timers::mpsc::recv_timeout(&completion, Duration::from_secs(5)).unwrap(),
        Err(StoreError::UpgradeCancelled { .. })
    ));
    assert!(store.list_documents().unwrap().is_empty());
    release.send(()).unwrap();
    operation.join().unwrap();
    waiter.join().unwrap();
    assert!(!store.attached_handles().unwrap().live[0].draining);
}

#[test]
fn starting_an_upgrade_inside_a_snapshot_is_refused_without_a_registry_change() {
    let (_, store) = fixture();
    let before = store.attached_handles().unwrap();
    store
        .snapshot(|_| {
            assert!(matches!(
                store.clone().drain_upgrade(),
                Err(StoreError::UpgradeFromOperation { .. })
            ));
        })
        .unwrap();
    assert_eq!(store.attached_handles().unwrap(), before);
}

#[test]
fn starting_an_upgrade_inside_a_commit_observer_is_refused() {
    let (_, store) = fixture();
    store
        .observe(Arc::new(RefuseDrainFromCommit(store.clone())))
        .unwrap();
    store
        .update_document(Swap::new(
            node(),
            document(),
            DocumentBody::new(b"x".to_vec()),
        ))
        .unwrap()
        .agent_stamp
        .unwrap();
    assert!(!store.attached_handles().unwrap().live[0].draining);
}

#[test]
fn an_independent_attachment_can_write_while_this_handle_drains() {
    let (vfs, store) = fixture();
    let drain = store.drain_upgrade().unwrap();
    let (other, _) = Store::open(
        vfs,
        store.root.clone(),
        StoreOptions::without_nonce_source(Agent::new("drain-tests", "test", "drain fixture", 1)),
    )
    .unwrap();
    other
        .write_document_replicated_as(
            node(),
            &document(),
            &DocumentBody::new(b"other".to_vec()),
            &Nonce::new("other").unwrap(),
        )
        .unwrap();
    assert!(drain.is_ready().unwrap());
    drain.cancel().unwrap();
    assert!(matches!(
        store.read_document(node(), &document()).unwrap(),
        crate::Read::Found(_)
    ));
}

#[test]
fn old_drain_guards_cannot_reopen_admission_during_a_later_drain() {
    let (_, store) = fixture();
    let first = store.drain_upgrade().unwrap();
    first.cancel().unwrap();
    let second = store.drain_upgrade().unwrap();
    drop(first);
    assert!(matches!(
        store.read_document(node(), &document()),
        Err(StoreError::AttachmentDraining { .. })
    ));
    drop(second);
    assert!(matches!(
        store.read_document(node(), &document()).unwrap(),
        crate::Read::Absent
    ));
}

#[test]
fn a_read_only_attachment_can_drain_and_cancel_without_writer_exclusion() {
    let (vfs, writer) = fixture();
    let reader = Store::open_read_only(
        vfs,
        writer.root.clone(),
        StoreOptions::without_nonce_source(Agent::new("drain-tests", "test", "drain fixture", 1)),
    )
    .unwrap();
    let count = writer.modifications().unwrap();
    let drain = reader.drain_upgrade().unwrap();
    drain.wait().unwrap();
    assert_eq!(writer.modifications().unwrap(), count);
    drain.cancel().unwrap();
    assert!(reader.list_documents().unwrap().is_empty());
}

#[test]
fn an_operation_lease_keeps_the_attachment_alive_after_all_store_clones_drop() {
    let (vfs, store) = fixture();
    let diagnostic = DiagnosticView::open(vfs, store.root.clone()).unwrap();
    let lease = store.operation_lease().unwrap();
    drop(store);
    assert_eq!(diagnostic.attached_handles().unwrap().live.len(), 1);
    drop(lease);
    assert!(diagnostic.attached_handles().unwrap().live.is_empty());
}

#[test]
fn a_registry_update_failure_does_not_undo_the_drain_or_detach_the_handle() {
    let gate = FaultGate::closed();
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::Write,
        PathMatcher::has_component("registry"),
        Fault::io_error(),
        When::Always,
        gate.clone(),
    );
    let (_, store) = fixture_with(policy);
    gate.open();
    let drain = store.drain_upgrade().unwrap();
    assert!(drain.registration_error().is_some());
    assert!(matches!(
        store.list_documents(),
        Err(StoreError::AttachmentDraining { .. })
    ));
    assert!(drain.cancel().is_err());
    gate.close();
    assert!(store.list_documents().unwrap().is_empty());
}

#[test]
fn a_finished_recovery_step_is_still_an_operation_on_its_attachment() {
    let (_, store) = fixture();
    let mut run = store.recovery().unwrap();
    while !run.progress().finished() {
        run.step().unwrap().agent_stamp.unwrap();
    }
    let drain = store.drain_upgrade().unwrap();
    assert!(matches!(
        run.step(),
        Err(StoreError::AttachmentDraining { .. })
    ));
    drop(drain);
    assert!(run.step().unwrap().finished());
}

#[test]
fn a_stalled_registry_update_does_not_hold_the_operation_admission_mutex() {
    let gate = FaultGate::closed();
    let (started, entered) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let released = Arc::new(Mutex::new(released));
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::Write,
        PathMatcher::has_component("registry"),
        Fault::observe_with(move |_, _| {
            started.send(()).unwrap();
            let _ = released.lock().unwrap().recv();
        }),
        When::Once,
        gate.clone(),
    );
    let (_, store) = fixture_with(policy);
    gate.open();
    let worker = store.clone();
    let registering = std::thread::spawn(move || worker.drain_upgrade().unwrap());
    entered.recv().unwrap();
    let (reported, report) = mpsc::channel();
    let reader = store.clone();
    let reading = std::thread::spawn(move || {
        let _ = reported.send(reader.list_documents());
    });
    let refusal = test_timers::mpsc::recv_timeout(&report, Duration::from_secs(5));
    release.send(()).unwrap();
    let drain = registering.join().unwrap();
    reading.join().unwrap();
    assert!(matches!(
        refusal.unwrap(),
        Err(StoreError::AttachmentDraining { .. })
    ));
    gate.close();
    drain.cancel().unwrap();
}

#[test]
fn a_writer_waiting_for_exclusion_is_already_leased_and_may_finish_during_drain() {
    let (_, store) = fixture();
    let (started, entered) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let worker = store.clone();
    let holder = std::thread::spawn(move || {
        let _guard = worker.writable().unwrap();
        started.send(()).unwrap();
        released.recv().unwrap();
    });
    entered.recv().unwrap();
    let worker = store.clone();
    let writer = std::thread::spawn(move || {
        let _guard = worker.writable().unwrap();
        worker
            .write_document_replicated(
                node(),
                &document(),
                &DocumentBody::new(b"waiting writer".to_vec()),
            )
            .unwrap()
            .agent_stamp
            .unwrap();
    });
    store.coordination.leases().wait_until_active(2);
    let drain = store.drain_upgrade().unwrap();
    assert!(!drain.is_ready().unwrap());
    release.send(()).unwrap();
    holder.join().unwrap();
    writer.join().unwrap();
    drain.wait().unwrap();
    drop(drain);
    assert!(matches!(
        store.read_document(node(), &document()).unwrap(),
        crate::Read::Found(_)
    ));
}

#[test]
fn a_drain_guard_created_elsewhere_cannot_wait_inside_an_existing_operation() {
    let (_, store) = fixture();
    let (started, entered) = mpsc::channel();
    let (deliver, delivered) = mpsc::channel::<Arc<UpgradeDrain>>();
    let worker = store.clone();
    let operation = std::thread::spawn(move || {
        worker
            .snapshot(|_| {
                started.send(()).unwrap();
                let drain = delivered.recv().unwrap();
                assert!(matches!(
                    drain.wait(),
                    Err(StoreError::UpgradeFromOperation { .. })
                ));
            })
            .unwrap()
    });
    entered.recv().unwrap();
    let drain = Arc::new(store.drain_upgrade().unwrap());
    deliver.send(Arc::clone(&drain)).unwrap();
    operation.join().unwrap();
    drain.wait().unwrap();
    drain.cancel().unwrap();
}

#[test]
fn a_commit_observer_cannot_start_a_drain_through_an_independent_attachment() {
    let (vfs, store) = fixture();
    let (other, _) = Store::open(
        vfs,
        store.root.clone(),
        StoreOptions::without_nonce_source(Agent::new("drain-tests", "test", "drain fixture", 1)),
    )
    .unwrap();
    store
        .observe(Arc::new(RefuseDrainFromCommit(other.clone())))
        .unwrap();
    store
        .update_document(Swap::new(
            node(),
            document(),
            DocumentBody::new(b"x".to_vec()),
        ))
        .unwrap()
        .agent_stamp
        .unwrap();
    assert!(
        !other
            .attached_handles()
            .unwrap()
            .live
            .iter()
            .any(|entry| entry.draining)
    );
}

#[test]
fn unwinding_a_callback_releases_its_operation_context_and_attachment_lease() {
    let (_, store) = fixture();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = store.snapshot(|_| panic!("the reader failed"));
        }))
        .is_err()
    );
    let drain = store.drain_upgrade().unwrap();
    assert!(drain.is_ready().unwrap());
    drain.wait().unwrap();
    drain.cancel().unwrap();
}
