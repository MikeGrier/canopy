// Copyright (c) Michael Grier.

//! API conformance to the opaque client contract composed by M14-7.

use std::collections::BTreeMap;
use std::time::Duration;

use super::super::*;
use crate::ring::{Command, CommandValue, Completion, RequestId, SubmissionRefusal};

const TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Effect {
    None,
    Applied,
    NotApplied,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Pending,
    Complete,
    Delivered,
    Unavailable,
    Refused,
}

#[derive(Debug, Clone)]
struct Request {
    pair: u64,
    id: u64,
    state: State,
    refusal: Option<SubmissionRefusal>,
    effect: Effect,
    recovery_required: bool,
}

#[derive(Default)]
struct PairProjection {
    live: bool,
    capacity: usize,
    next: u64,
}

/// The public projection only: no queue, worker, notification or started state.
struct ClientOracle {
    pairs: BTreeMap<u64, PairProjection>,
    requests: Vec<Request>,
}

impl ClientOracle {
    fn new() -> Self {
        Self {
            pairs: BTreeMap::new(),
            requests: Vec::new(),
        }
    }

    fn open(&mut self, pair: u64, capacity: usize) {
        assert!(capacity > 0);
        self.pairs.insert(
            pair,
            PairProjection {
                live: true,
                capacity,
                next: 1,
            },
        );
    }

    fn outstanding(&self, pair: u64) -> usize {
        self.requests
            .iter()
            .filter(|request| {
                request.pair == pair && matches!(request.state, State::Pending | State::Complete)
            })
            .count()
    }

    fn submit(
        &mut self,
        pair: u64,
        observed: Result<RequestId, SubmissionRefusal>,
    ) -> Result<(), String> {
        let outstanding = self.outstanding(pair);
        let pair_state = self.pairs.get_mut(&pair).expect("known pair");
        let refusal = if !pair_state.live {
            Some(SubmissionRefusal::PairClosed)
        } else if outstanding >= pair_state.capacity {
            Some(SubmissionRefusal::CompletionQueueFull)
        } else {
            None
        };
        match (refusal, observed) {
            (Some(expected), Err(actual)) if expected == actual => {
                self.requests.push(Request {
                    pair,
                    id: 0,
                    state: State::Refused,
                    refusal: Some(actual),
                    effect: Effect::None,
                    recovery_required: false,
                });
                Ok(())
            }
            (None, Ok(actual)) if actual.get() == pair_state.next => {
                self.requests.push(Request {
                    pair,
                    id: actual.get(),
                    state: State::Pending,
                    refusal: None,
                    effect: Effect::None,
                    recovery_required: false,
                });
                pair_state.next += 1;
                Ok(())
            }
            (expected, actual) => Err(format!(
                "submission projection disagrees for pair {pair}: expected refusal {expected:?}, observed {actual:?}"
            )),
        }
    }

    fn complete(
        &mut self,
        pair: u64,
        request: RequestId,
        effect: Effect,
        recovery_required: bool,
    ) -> Result<(), String> {
        let id = request.get();
        let next = self
            .requests
            .iter()
            .filter(|candidate| candidate.pair == pair && candidate.state == State::Pending)
            .map(|candidate| candidate.id)
            .min();
        if next != Some(id) {
            return Err(format!(
                "completion {id} for pair {pair} overtook pending {next:?}"
            ));
        }
        let index = self.request_index(pair, request)?;
        let request = &mut self.requests[index];
        if !matches!(
            effect,
            Effect::Applied | Effect::NotApplied | Effect::Unknown
        ) {
            return Err("a completed request needs an effect class".to_owned());
        }
        if recovery_required && effect != Effect::Unknown {
            return Err("recovery can accompany only uncertain effect".to_owned());
        }
        request.state = State::Complete;
        request.effect = effect;
        request.recovery_required = recovery_required;
        Ok(())
    }

    fn deliver(&mut self, pair: u64, request: RequestId) -> Result<(), String> {
        let index = self.request_index(pair, request)?;
        let pair = self.requests[index].pair;
        if self.requests[index].state != State::Complete {
            return Err(format!("request {request:?} is not a completion"));
        }
        if !self.pairs[&pair].live {
            return Err("a closed pair delivered a completion".to_owned());
        }
        self.requests[index].state = State::Delivered;
        Ok(())
    }

    fn close(&mut self, pair: u64) {
        self.pairs.get_mut(&pair).expect("known pair").live = false;
        for request in &mut self.requests {
            if request.pair == pair && matches!(request.state, State::Pending | State::Complete) {
                request.state = State::Unavailable;
                request.effect = Effect::Unknown;
                request.recovery_required = false;
            }
        }
    }

    fn settle_unavailable(
        &mut self,
        pair: u64,
        request: RequestId,
        recovery_required: bool,
    ) -> Result<(), String> {
        let index = self.request_index(pair, request)?;
        let entry = &mut self.requests[index];
        if entry.state != State::Unavailable {
            return Err(format!("request {request:?} is not unavailable"));
        }
        entry.effect = Effect::Unknown;
        entry.recovery_required = recovery_required;
        Ok(())
    }

    fn request_index(&self, pair: u64, request: RequestId) -> Result<usize, String> {
        self.requests
            .iter()
            .position(|candidate| {
                candidate.pair == pair
                    && candidate.id == request.get()
                    && candidate.state != State::Refused
            })
            .ok_or_else(|| format!("request {request:?} is not accepted for pair {pair}"))
    }

    fn recover(&mut self) {
        for request in &mut self.requests {
            request.recovery_required = false;
        }
        self.assert_promises();
    }

    fn effect(&self, pair: u64, request: RequestId) -> Effect {
        self.requests[self.request_index(pair, request).unwrap()].effect
    }

    fn assert_promises(&self) {
        for request in &self.requests {
            if request.state == State::Unavailable {
                assert_eq!(request.effect, Effect::Unknown);
            }
            if request.recovery_required {
                assert_eq!(request.effect, Effect::Unknown);
            }
            if request.state == State::Refused {
                assert_eq!(request.id, 0);
                assert!(request.refusal.is_some());
            }
        }
        for pair in self.pairs.keys() {
            assert!(self.outstanding(*pair) <= self.pairs[pair].capacity);
        }
    }
}

fn observe_completion(
    oracle: &mut ClientOracle,
    store: &Store,
    pair: u64,
    completion: Completion,
) -> Result<(), String> {
    let (request, outcome) = completion.into_parts();
    let store_recovery_required = store
        .recovery_required()
        .map_err(|error| error.to_string())?;
    let (effect, recovery_required) = match outcome {
        Ok(CommandValue::Transaction(outcome)) => (
            if outcome.is_committed() {
                Effect::Applied
            } else {
                Effect::NotApplied
            },
            false,
        ),
        Ok(
            CommandValue::Read(_)
            | CommandValue::Children(_)
            | CommandValue::Documents(_)
            | CommandValue::Snapshot(_),
        ) => (Effect::NotApplied, false),
        Err(_) if store_recovery_required => (Effect::Unknown, true),
        Err(_) => (Effect::NotApplied, false),
    };
    oracle.complete(pair, request, effect, recovery_required)?;
    oracle.deliver(pair, request)?;
    oracle.assert_promises();
    Ok(())
}

#[test]
fn ordinary_commit_and_rejection_refine_the_client_projection() {
    let (_vfs, store) = sourced_store();
    let _ = store
        .update_document(Swap::new(name("alpha"), doc("guard"), body(b"old")))
        .unwrap();
    let session = store.session().unwrap();
    let (mut submissions, mut completions) = session.queue_pair(2).unwrap();
    let mut oracle = ClientOracle::new();
    oracle.open(1, 2);

    let mut committed = store.transaction();
    committed.write(name("alpha"), doc("one"), body(b"one"));
    let committed = submissions
        .submit(Command::Transaction(
            committed.build().unwrap(),
            crate::TransactionOptions,
        ))
        .map_err(|error| match error {
            crate::ring::SubmitError::Refused(refusal) => refusal,
            crate::ring::SubmitError::Doorbell(_) => panic!("doorbell failed"),
        });
    oracle.submit(1, committed).unwrap();

    let mut rejected = store.transaction();
    rejected.write(name("alpha"), doc("two"), body(b"two"));
    rejected
        .read_document(name("alpha"), &doc("guard"))
        .unwrap();
    let stale = rejected.build().unwrap();
    let _ = store
        .update_document(Swap::new(name("alpha"), doc("guard"), body(b"new")))
        .unwrap();
    let rejected = submissions
        .submit(Command::Transaction(stale, crate::TransactionOptions))
        .map_err(|error| match error {
            crate::ring::SubmitError::Refused(refusal) => refusal,
            crate::ring::SubmitError::Doorbell(_) => panic!("doorbell failed"),
        });
    oracle.submit(1, rejected).unwrap();

    observe_completion(
        &mut oracle,
        &store,
        1,
        completions.wait(TIMEOUT).unwrap().unwrap(),
    )
    .unwrap();
    observe_completion(
        &mut oracle,
        &store,
        1,
        completions.wait(TIMEOUT).unwrap().unwrap(),
    )
    .unwrap();
    assert_eq!(text(&store, "alpha", "one").as_deref(), Some("one"));
    assert_eq!(text(&store, "alpha", "two"), None);
}

#[test]
fn full_capacity_and_closed_pair_refuse_without_request_identity() {
    let (_vfs, store) = sourced_store();
    let session = store.session().unwrap();
    let (mut submissions, completions) = session.queue_pair(1).unwrap();
    let mut oracle = ClientOracle::new();
    oracle.open(1, 1);
    let first = submissions.push(Command::ListDocuments);
    oracle.submit(1, first).unwrap();
    let second = submissions.push(Command::ListDocuments);
    oracle.submit(1, second).unwrap();
    drop(completions);
    oracle.close(1);
    let closed = submissions.push(Command::ListDocuments);
    oracle.submit(1, closed).unwrap();
    oracle.assert_promises();
    assert!(matches!(
        second,
        Err(SubmissionRefusal::CompletionQueueFull)
    ));
    assert!(matches!(closed, Err(SubmissionRefusal::PairClosed)));
}

#[test]
fn an_unnotified_pair_loss_withdraws_without_visible_effect() {
    let (_vfs, store) = sourced_store();
    let session = store.session().unwrap();
    let (mut submissions, completions) = session.queue_pair(1).unwrap();
    let mut oracle = ClientOracle::new();
    oracle.open(1, 1);
    let mut builder = store.transaction();
    builder.write(name("alpha"), doc("one"), body(b"withdrawn"));
    let request = submissions.push(Command::Transaction(
        builder.build().unwrap(),
        crate::TransactionOptions,
    ));
    oracle.submit(1, request).unwrap();
    drop((submissions, completions));
    oracle.close(1);
    test_timers::thread::sleep(Duration::from_millis(30));
    oracle.assert_promises();
    assert_eq!(text(&store, "alpha", "one"), None);
}

#[test]
fn a_parked_head_does_not_block_a_ready_pair() {
    let (_vfs, store) = sourced_store();
    let session = store.session().unwrap();
    let (mut blocked_submit, mut blocked_complete) = session.queue_pair(1).unwrap();
    let (mut ready_submit, mut ready_complete) = session.queue_pair(1).unwrap();
    let mut oracle = ClientOracle::new();
    oracle.open(1, 1);
    oracle.open(2, 1);
    let guard = store.exclusive();
    let mut builder = store.transaction();
    builder.write(name("alpha"), doc("one"), body(b"later"));
    let blocked = blocked_submit.submit(Command::Transaction(
        builder.build().unwrap(),
        crate::TransactionOptions,
    ));
    let blocked = blocked.map_err(|error| match error {
        crate::ring::SubmitError::Refused(reason) => reason,
        crate::ring::SubmitError::Doorbell(_) => panic!("doorbell failed"),
    });
    oracle.submit(1, blocked).unwrap();
    let ready = ready_submit
        .submit(Command::ListDocuments)
        .map_err(|error| match error {
            crate::ring::SubmitError::Refused(reason) => reason,
            crate::ring::SubmitError::Doorbell(_) => panic!("doorbell failed"),
        });
    oracle.submit(2, ready).unwrap();
    observe_completion(
        &mut oracle,
        &store,
        2,
        ready_complete.wait(TIMEOUT).unwrap().unwrap(),
    )
    .unwrap();
    assert!(blocked_complete.try_pop().is_none());
    drop(guard);
    observe_completion(
        &mut oracle,
        &store,
        1,
        blocked_complete.wait(TIMEOUT).unwrap().unwrap(),
    )
    .unwrap();
}

struct BlockingCommitObserver {
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}

impl crate::CommitObserver for BlockingCommitObserver {
    fn committed(&self, _node: &NodePath, _document: &DocumentName, _body: &DocumentBody) {
        self.entered.send(()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
    }
}

#[test]
fn a_started_request_finishes_unavailable_after_pair_loss() {
    let (_vfs, store) = sourced_store();
    let (entered_send, entered) = std::sync::mpsc::channel();
    let (release_send, release) = std::sync::mpsc::channel();
    store
        .observe(std::sync::Arc::new(BlockingCommitObserver {
            entered: entered_send,
            release: std::sync::Mutex::new(release),
        }))
        .unwrap();
    let session = store.session().unwrap();
    let (mut submissions, completions) = session.queue_pair(1).unwrap();
    let mut oracle = ClientOracle::new();
    oracle.open(1, 1);
    let mut builder = store.transaction();
    builder.write(name("alpha"), doc("one"), body(b"committed"));
    let request = submissions
        .submit(Command::Transaction(
            builder.build().unwrap(),
            crate::TransactionOptions,
        ))
        .map_err(|error| match error {
            crate::ring::SubmitError::Refused(reason) => reason,
            crate::ring::SubmitError::Doorbell(_) => panic!("doorbell failed"),
        });
    oracle.submit(1, request).unwrap();
    let request = request.unwrap();
    test_timers::mpsc::recv_timeout(&entered, TIMEOUT).unwrap();
    drop((submissions, completions));
    oracle.close(1);
    release_send.send(()).unwrap();
    let deadline = std::time::Instant::now() + TIMEOUT;
    while text(&store, "alpha", "one").as_deref() != Some("committed") {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    oracle.settle_unavailable(1, request, false).unwrap();
    oracle.assert_promises();
    assert_eq!(oracle.effect(1, request), Effect::Unknown);
}

#[test]
fn an_after_prepare_failure_completes_as_uncertain_and_stays_uncertain_after_recovery() {
    use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, MemFs, When};

    let gate = FaultGate::closed();
    let faulty = Arc::new(FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Rename,
            namespace_tree_staging(),
            Fault::io_error(),
            When::Once,
            gate.clone(),
        ),
    ));
    let vfs: Arc<dyn Vfs> = faulty.clone();
    let root = PathBuf::from("client-contract-fault");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init(
        Arc::clone(&vfs),
        root.clone(),
        test_config(),
        StoreOptions::with_nonce_source(test_agent(), counting_nonce_source()),
    )
    .unwrap();
    let session = store.session().unwrap();
    let (mut submissions, mut completions) = session.queue_pair(1).unwrap();
    let mut oracle = ClientOracle::new();
    oracle.open(1, 1);
    let mut builder = store.transaction();
    builder.write(name("alpha"), doc("one"), body(b"maybe"));
    let request = submissions
        .submit(Command::Transaction(
            builder.build().unwrap(),
            crate::TransactionOptions,
        ))
        .map_err(|error| match error {
            crate::ring::SubmitError::Refused(reason) => reason,
            crate::ring::SubmitError::Doorbell(_) => panic!("doorbell failed"),
        });
    oracle.submit(1, request).unwrap();
    let request = request.unwrap();
    gate.open();
    let completion = completions.wait(TIMEOUT).unwrap().unwrap();
    gate.close();
    observe_completion(&mut oracle, &store, 1, completion).unwrap();
    assert!(store.recovery_required().unwrap());
    assert_eq!(oracle.effect(1, request), Effect::Unknown);

    drop(completions);
    drop(submissions);
    drop(session);
    drop(store);
    faulty.inner().simulate_crash();
    let (reopened, _) = Store::open(
        vfs,
        root,
        StoreOptions::with_nonce_source(test_agent(), counting_nonce_source()),
    )
    .unwrap();
    let recovery = reopened.recover().unwrap();
    let _ = recovery;
    assert_eq!(text(&reopened, "alpha", "one").as_deref(), Some("maybe"));
    oracle.recover();
    assert_eq!(oracle.effect(1, request), Effect::Unknown);
}

#[test]
fn continuously_ready_pairs_keep_pair_local_order_and_all_make_progress() {
    let (_vfs, store) = sourced_store();
    let session = store.session().unwrap();
    let pair_count = 8u64;
    let mut oracle = ClientOracle::new();
    let mut pairs = Vec::new();
    for pair in 1..=pair_count {
        oracle.open(pair, 3);
        let (mut submissions, completions) = session.queue_pair(3).unwrap();
        for _ in 0..3 {
            let request = submissions
                .submit(Command::ListDocuments)
                .map_err(|error| match error {
                    crate::ring::SubmitError::Refused(reason) => reason,
                    crate::ring::SubmitError::Doorbell(_) => panic!("doorbell failed"),
                });
            oracle.submit(pair, request).unwrap();
        }
        pairs.push((pair, submissions, completions));
    }
    for (pair, submissions, mut completions) in pairs {
        for _ in 0..3 {
            observe_completion(
                &mut oracle,
                &store,
                pair,
                completions.wait(TIMEOUT).unwrap().unwrap(),
            )
            .unwrap();
        }
        drop(submissions);
    }
    oracle.assert_promises();
}

#[test]
fn independent_store_views_contending_on_appends_keep_every_byte() {
    use acadia::MemFs;

    let mem = MemFs::new();
    let root = PathBuf::from("client-contract-multi-store");
    mem.create_dir_all(&root).unwrap();
    let init_vfs: Arc<dyn Vfs> = Arc::new(mem.another_process());
    let first = Store::init(
        Arc::clone(&init_vfs),
        root.clone(),
        test_config(),
        StoreOptions::with_nonce_source(test_agent(), counting_nonce_source()),
    )
    .unwrap();
    let (second, _) = Store::open(
        Arc::new(mem.another_process()),
        root,
        StoreOptions::with_nonce_source(test_agent(), counting_nonce_source()),
    )
    .unwrap();
    let stores = [first, second];
    let (completed_send, completed) = std::sync::mpsc::channel();
    thread::scope(|scope| {
        for (index, store) in stores.into_iter().enumerate() {
            let completed_send = completed_send.clone();
            scope.spawn(move || {
                let session = store.session().unwrap();
                let (mut submissions, mut completions) = session.queue_pair(1).unwrap();
                for _ in 0..32 {
                    let mut builder = store.transaction();
                    builder.append(name("alpha"), doc("log"), vec![index as u8]);
                    let request = submissions
                        .submit(Command::Transaction(
                            builder.build().unwrap(),
                            crate::TransactionOptions,
                        ))
                        .unwrap();
                    let completion = completions.wait(TIMEOUT).unwrap().unwrap();
                    assert_eq!(completion.request(), request);
                    completed_send.send((index as u64 + 1, completion)).unwrap();
                }
            });
        }
    });
    drop(completed_send);
    let first = store_handle(&mem, "client-contract-multi-store");
    let mut oracle = ClientOracle::new();
    oracle.open(1, 1);
    oracle.open(2, 1);
    for (pair, completion) in completed {
        let request = completion.request();
        oracle.submit(pair, Ok(request)).unwrap();
        observe_completion(&mut oracle, &first, pair, completion).unwrap();
    }
    let body = first
        .read_body(name("alpha"), &doc("log"))
        .unwrap()
        .unwrap();
    assert_eq!(body.as_bytes().len(), 64);
    assert_eq!(
        body.as_bytes().iter().filter(|&&byte| byte == 0).count(),
        32
    );
    assert_eq!(
        body.as_bytes().iter().filter(|&&byte| byte == 1).count(),
        32
    );
}

fn store_handle(mem: &MemFs, root: &str) -> Store {
    Store::open(
        Arc::new(mem.another_process()),
        PathBuf::from(root),
        StoreOptions::with_nonce_source(test_agent(), counting_nonce_source()),
    )
    .unwrap()
    .0
}

#[test]
fn a_failure_before_prepare_completes_with_definite_no_effect() {
    use acadia::{Fault, FaultOp, FaultPolicy, FaultyFs, MemFs, When};

    let root = PathBuf::from("client-contract-before-prepare");
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject(
            FaultOp::Append,
            root.join(".commit-log"),
            Fault::io_error(),
            When::OnCall(1),
        ),
    ));
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init(
        vfs,
        root,
        test_config(),
        StoreOptions::with_nonce_source(test_agent(), counting_nonce_source()),
    )
    .unwrap();
    let session = store.session().unwrap();
    let (mut submissions, mut completions) = session.queue_pair(1).unwrap();
    let mut oracle = ClientOracle::new();
    oracle.open(1, 1);
    let mut builder = store.transaction();
    builder.write(name("alpha"), doc("one"), body(b"not committed"));
    let request = submissions
        .submit(Command::Transaction(
            builder.build().unwrap(),
            crate::TransactionOptions,
        ))
        .map_err(|error| match error {
            crate::ring::SubmitError::Refused(reason) => reason,
            crate::ring::SubmitError::Doorbell(_) => panic!("doorbell failed"),
        });
    oracle.submit(1, request).unwrap();
    let request = request.unwrap();
    observe_completion(
        &mut oracle,
        &store,
        1,
        completions.wait(TIMEOUT).unwrap().unwrap(),
    )
    .unwrap();
    assert!(!store.recovery_required().unwrap());
    assert_eq!(text(&store, "alpha", "one"), None);
    assert_eq!(oracle.effect(1, request), Effect::NotApplied);
}

#[test]
fn the_client_oracle_rejects_out_of_order_and_known_loss() {
    let (_vfs, store) = sourced_store();
    let session = store.session().unwrap();
    let (mut submissions, completions) = session.queue_pair(2).unwrap();
    let first = submissions.push(Command::ListDocuments).unwrap();
    let second = submissions.push(Command::ListDocuments).unwrap();
    let mut oracle = ClientOracle::new();
    oracle.open(1, 2);
    oracle.submit(1, Ok(first)).unwrap();
    oracle.submit(1, Ok(second)).unwrap();
    assert!(
        oracle
            .complete(1, second, Effect::NotApplied, false)
            .is_err()
    );
    oracle.complete(1, first, Effect::Applied, false).unwrap();
    oracle.close(1);
    assert_eq!(oracle.effect(1, first), Effect::Unknown);
    assert_eq!(oracle.effect(1, second), Effect::Unknown);
    oracle.assert_promises();
    drop((submissions, completions));
}
